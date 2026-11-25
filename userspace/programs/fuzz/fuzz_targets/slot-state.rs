// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The blocks GRUB and the system share on the EFI system partition: the
//! slot state's copy, a try of `changes bisect`, and the environment
//! block under both. A block that reads is written back and read as it
//! was.

#![forbid(unsafe_code)]
#![no_main]

use libfuzzer_sys::fuzz_target;
use slots::envblock;
use slots::state::State;
use slots::trial::Trial;

fuzz_target!(|data: &[u8]| {
    let _ = envblock::parse(data);
    let state = State::decode(data);
    if let Some(state) = state {
        assert_eq!(State::decode(&state.encode()), Some(state));
    }
    if let Some((trial, _)) = Trial::decode(data) {
        // A try as a block holds it is not always one `Trial::new` makes.
        if let Ok(made) = Trial::new(trial.number, trial.slot, &trial.options) {
            assert_eq!(Trial::decode(&made.encode()), Some((made, false)));
        }
    }
});
