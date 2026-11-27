// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Kani proof harnesses (ADR 0623): for every program of up to
//! [`LONGEST`] instructions, whatever their bits, and every call, the
//! checks never panic; a checked program runs without a panic or an
//! index out of bounds and ends; a call the cache would skip the filter
//! for is one the program allows whatever its arguments; and a filter
//! added to a chain never makes its verdict less strict. `ob test kani`
//! runs them. Longer programs are the host tests' and the fuzz target's.
//!
//! Their unwind bound, 6, covers the loops over a program of at most
//! [`LONGEST`] instructions, a run's steps among them: the checks look
//! codes up in sets (`machine::Codes`) rather than search lists, which
//! would be loops of up to 16.

use crate::calls::allows_whatever_the_arguments;
use crate::filter::Program;
use crate::machine::Instruction;
use crate::seccomp::{RET_ALLOW, SeccompData, action_only, stricter};

/// The longest program the proofs take.
const LONGEST: usize = 4;

/// Any program of up to [`LONGEST`] instructions.
fn any_program() -> ([Instruction; LONGEST], usize) {
    let instructions = core::array::from_fn(|_| Instruction {
        code: kani::any(),
        jt: kani::any(),
        jf: kani::any(),
        k: kani::any(),
    });
    (instructions, kani::any_where(|&len| len <= LONGEST))
}

/// Any call.
fn any_call() -> SeccompData {
    SeccompData {
        nr: kani::any(),
        arch: kani::any(),
        instruction_pointer: kani::any(),
        args: kani::any(),
    }
}

/// The checks take any bits without a panic.
#[kani::proof]
#[kani::unwind(6)]
fn the_checks_take_any_program() {
    let (instructions, len) = any_program();
    let mut masks = [0u16; LONGEST];
    let _ = Program::check(&instructions[..len], &mut masks);
}

/// A checked program runs to a RET for any call: no panic, no index out of
/// bounds, and no loop past the unwind bound.
#[kani::proof]
#[kani::unwind(6)]
fn a_checked_program_always_ends() {
    let (instructions, len) = any_program();
    let mut masks = [0u16; LONGEST];
    let Ok(program) = Program::check(&instructions[..len], &mut masks) else { return };
    let _ = program.run(&any_call());
}

/// A call the cache skips the filter for gets ALLOW from the program,
/// whatever its arguments and instruction pointer.
#[kani::proof]
#[kani::unwind(6)]
fn the_cache_skips_only_calls_the_program_allows() {
    let (instructions, len) = any_program();
    let mut masks = [0u16; LONGEST];
    let Ok(program) = Program::check(&instructions[..len], &mut masks) else { return };
    let data = any_call();
    if allows_whatever_the_arguments(program.instructions(), data.nr.cast_unsigned(), data.arch) {
        assert_eq!(program.run(&data), RET_ALLOW);
    }
}

/// The verdict of a chain with one more filter is as strict as the
/// chain's was, and as strict as the new filter's.
#[kani::proof]
fn a_filter_added_never_loosens_the_chain() {
    let (kept, next): (u32, u32) = (kani::any(), kani::any());
    let verdict = stricter(kept, next);
    assert!(action_only(verdict) <= action_only(kept));
    assert!(action_only(verdict) <= action_only(next));
    assert!(verdict == kept || verdict == next);
}
