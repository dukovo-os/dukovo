// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A seccomp filter's program: the machine's ([`machine`]), checked as
//! Linux checks it (`bpf_check_classic` in net/core/filter.c, then
//! `seccomp_check_filter` in kernel/seccomp.c) and run on the call's
//! [`SeccompData`]. It ends with a `RET` whose value is the action
//! (`seccomp`).
//!
//! [`machine`]: crate::machine

use crate::machine::opcode::{
    A, ABS, ADD, ALU, AND, DIV, IMM, JA, JEQ, JGE, JGT, JMP, JSET, K, LD, LDX, LEN, LSH, MEM, MISC,
    MOD, MUL, NEG, OR, RET, RSH, ST, STX, SUB, TAX, TXA, W, X, XOR,
};
use crate::machine::{self, Instruction, Invalid, is_test};
use crate::seccomp::{SECCOMP_DATA_SIZE, SeccompData};

/// A program that passed Linux's checks, in storage `S`: an array, or the
/// kernel's vector. Only [`Program::check`] makes one, so every program
/// that runs is a checked one.
#[derive(Clone, Debug)]
pub struct Program<S> {
    instructions: S,
}

impl<S: AsRef<[Instruction]>> Program<S> {
    /// Checks `instructions` as Linux's `bpf_check_classic` and
    /// `seccomp_check_filter` do. `masks` is room for the check of the
    /// memory words, one for each instruction (the kernel's stack has no
    /// room for 4,096 of them).
    ///
    /// # Errors
    ///
    /// Why it was refused: the first fault in Linux's order of checks.
    /// [`Invalid::Length`] also when `masks` is shorter than the program.
    pub fn check(instructions: S, masks: &mut [u16]) -> Result<Program<S>, Invalid> {
        let program = instructions.as_ref();
        machine::check(program)?;
        let masks = masks.get_mut(..program.len()).ok_or(Invalid::Length)?;
        check_memory(program, masks)?;
        check_seccomp(program)?;
        Ok(Program { instructions })
    }

    /// The instructions, as checked.
    pub fn instructions(&self) -> &[Instruction] {
        self.instructions.as_ref()
    }

    /// How many instructions it has.
    pub fn len(&self) -> usize {
        self.instructions().len()
    }

    /// Never: a checked program has an instruction at least. For Clippy's
    /// rule that a `len` has its `is_empty`.
    pub fn is_empty(&self) -> bool {
        self.instructions().is_empty()
    }

    /// How many instructions Linux's conversion of the program to eBPF
    /// makes (`bpf_convert_filter` in net/core/filter.c): what Linux's
    /// limit on the length of a chain of filters counts, so that a chain is
    /// refused here where it is on Linux. Three come first (A and X set to
    /// 0, and the context kept); then each instruction makes one, but a
    /// `RET` of a constant two (a move and the exit), a division by `X`
    /// five (a check for 0 that returns 0), a test of a constant with its
    /// top bit set one more (a move to a scratch register), and a test that
    /// jumps for both answers one more (a jump after it), unless it is a
    /// `JEQ`, `JGT` or `JGE` that goes on when true, which Linux turns
    /// around. The counts were measured on Linux 6.18.
    pub fn converted_len(&self) -> u32 {
        3 + self.instructions().iter().map(|&instruction| converted(instruction)).sum::<u32>()
    }

    /// The value the program returns for the call `data`: the action and
    /// its data (`seccomp::Action::of`). A division by an `X` of 0
    /// returns 0, as Linux's converted program does, which kills the
    /// thread.
    pub fn run(&self, data: &SeccompData) -> u32 {
        machine::run(self.instructions(), data)
    }
}

/// How many eBPF instructions Linux makes of `instruction`
/// ([`Program::converted_len`]).
fn converted(instruction: Instruction) -> u32 {
    let Instruction { code, jt, jf, k } = instruction;
    match code {
        c if c == RET | K => 2,
        c if c == ALU | DIV | X || c == ALU | MOD | X => 5,
        c if is_test(c) => {
            let moved = u32::from(c & X == 0 && k.cast_signed() < 0);
            let turned = jt == 0 && [JEQ, JGT, JGE].contains(&(c & 0xf0));
            moved + if jf == 0 || turned { 1 } else { 2 }
        }
        _ => 1,
    }
}

/// Linux's `check_load_and_stores`: each memory word is stored on every
/// path to each load of it. A jump hands the words stored so far to its
/// targets; the instruction after a jump starts with every word valid, as
/// only jumps reach it.
fn check_memory(program: &[Instruction], masks: &mut [u16]) -> Result<(), Invalid> {
    masks.fill(u16::MAX);
    let mut valid = 0u16;
    for (pc, instruction) in program.iter().enumerate() {
        let Instruction { code, jt, jf, k } = *instruction;
        valid &= masks[pc];
        // `machine::check` kept k below 16 for these four.
        let word = 1u16 << (k % 16);
        match code {
            c if c == ST || c == STX => valid |= word,
            c if c == LD | MEM || c == LDX | MEM => {
                if valid & word == 0 {
                    return Err(Invalid::Unset(pc));
                }
            }
            c if c == JMP | JA => {
                masks[pc + 1 + k as usize] &= valid;
                valid = u16::MAX;
            }
            c if is_test(c) => {
                masks[pc + 1 + usize::from(jt)] &= valid;
                masks[pc + 1 + usize::from(jf)] &= valid;
                valid = u16::MAX;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Linux's `seccomp_check_filter`: only the opcodes it lists, and loads
/// of a whole word inside `struct seccomp_data`. (Linux rewrites the
/// loads of the data's length as loads of the constant 64; here the
/// data's length is 64.)
fn check_seccomp(program: &[Instruction]) -> Result<(), Invalid> {
    for (pc, instruction) in program.iter().enumerate() {
        let allowed = if instruction.code == LD | W | ABS {
            (instruction.k as usize) < SECCOMP_DATA_SIZE && instruction.k % 4 == 0
        } else {
            is_allowed(instruction.code)
        };
        if !allowed {
            return Err(Invalid::Instruction(pc));
        }
    }
    Ok(())
}

/// Whether `code` is one of the other opcodes `seccomp_check_filter`
/// lists: no load of a byte, a half word or at an index, and no `MOD`,
/// though the machine has them.
fn is_allowed(code: u16) -> bool {
    const ARITHMETIC: [u16; 9] = [ADD, SUB, MUL, DIV, AND, OR, XOR, LSH, RSH];
    const TESTS: [u16; 4] = [JEQ, JGE, JGT, JSET];
    const OTHERS: [u16; 14] = [
        LD | W | LEN,
        LDX | W | LEN,
        LD | IMM,
        LDX | IMM,
        LD | MEM,
        LDX | MEM,
        ST,
        STX,
        ALU | NEG,
        MISC | TAX,
        MISC | TXA,
        JMP | JA,
        RET | K,
        RET | A,
    ];
    OTHERS.contains(&code)
        || ARITHMETIC.iter().any(|&op| code == ALU | op | K || code == ALU | op | X)
        || TESTS.iter().any(|&op| code == JMP | op | K || code == JMP | op | X)
}
