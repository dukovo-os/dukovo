// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `display-test panic`: the kernel panics while a program has the
//! screens. The test shows a picture on every screen, and at the one
//! step, `shown`, asks the kernel for a test panic (radix-crashdump(5)).
//! The panic report then shows on the screen of the card the kernel's
//! log is on, over the program's picture, which the host reads.

use std::io::Write;

use display::abi::MAY_BLANK;

use crate::rig::{Keys, Outcome, Seat};
use crate::stage::Stage;

/// Where a panic is asked for, and the word that asks.
const CRASH: &str = "/devices/crashdump";

/// Runs the test. It does not come back if the kernel does as asked.
pub fn run() -> Outcome {
    let mut keys = Keys::open()?;
    let mut stage = Stage::of(Seat::open()?, 0)?;
    stage.show_all(MAY_BLANK)?;
    keys.step("shown")?;
    let asked = std::fs::File::options().write(true).open(CRASH);
    let mut asked = asked.map_err(|err| format!("{CRASH}: {err}"))?;
    asked.write_all(b"panic\n").map_err(|err| format!("asking for a panic: {err}"))?;
    Err("the kernel ran on after a panic was asked for".into())
}
