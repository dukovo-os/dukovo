// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `display-test`: the display layer (radix-display(2)) on real cards,
//! for the display boot tests.
//!
//! ```text
//! display-test virtio | beat | handover | panic | cost
//! ```
//!
//! Each word but the last is one test, which a boot test of the same
//! name starts as root on a machine made for it:
//!
//! - `virtio`: a virtio graphics card of four screens, whose monitors
//!   the host plugs in, resizes and pulls out ([`virtio`]).
//! - `beat`: the beat of each card's pipe, and when a change shows
//!   ([`beat`]).
//! - `handover`: the screens borrowed from consoled, lent on, given back
//!   and left behind by a program that ends ([`handover`]).
//! - `panic`: a kernel panic while a program has the screens
//!   ([`panic`]).
//!
//! `cost` is for `ob bench`: it prints what a change, a check and a
//! pointer's move cost on each card ([`cost`]), and waits for no key.
//!
//! The program draws test patterns and says in the log what each screen
//! shows (the `display-pattern` library); at each step it waits for a key,
//! which the host presses once it has compared a picture of every
//! screen with what was said. It ends with `display-test: passed` and
//! status 0, or with `display-test: FAILED:` and why, and status 1.

#![forbid(unsafe_code)]

mod beat;
mod cost;
mod handover;
mod panic;
mod rig;
mod stage;
mod virtio;

use std::process::ExitCode;

use display_pattern::said::Said;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let outcome = match words[..] {
        ["virtio"] => virtio::run(),
        ["beat"] => beat::run(),
        ["handover"] => handover::run(),
        ["borrow", seed] => handover::borrow(seed),
        ["panic"] => panic::run(),
        ["cost"] => cost::run(),
        _ => Err("usage: display-test virtio | beat | handover | panic | cost".into()),
    };
    match outcome {
        Ok(()) => {
            rig::say(&Said::Passed);
            ExitCode::SUCCESS
        }
        Err(why) => {
            rig::say(&Said::Failed(why));
            ExitCode::FAILURE
        }
    }
}
