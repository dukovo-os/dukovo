// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The calls a filter allows whatever their arguments: Linux's seccomp
//! cache (5.11, `seccomp_cache_prepare` in kernel/seccomp.c). Most
//! filters allow most calls by number alone, and a thread whose chain
//! allows a call that way makes it without running any filter.
//!
//! [`Program::always_allows`] runs the program for each call number with
//! only the number and the arch known, as Linux's `seccomp_is_const_allow`
//! does: a load of anything else, or an instruction the walk does not
//! follow, leaves the call out. A chain's map is its filters' maps met
//! ([`Calls::meet`]), so a call is in it only if every filter allows it.

use crate::filter::Program;
use crate::machine::Instruction;
use crate::machine::opcode::{ABS, ALU, AND, JA, JEQ, JGE, JGT, JMP, JSET, K, LD, RET, W};

/// The call numbers the map holds: x86-64 Linux has fewer than 512, and a
/// call past them runs the filters.
pub const CALLS: usize = 512;

/// The offsets of `nr` and `arch` in `struct seccomp_data`.
const NR: u32 = 0;
const ARCH: u32 = 4;

/// The 64-bit words of a [`Calls`].
pub const WORDS: usize = CALLS / 64;

/// A set of call numbers below [`CALLS`]: bit `n % 64` of word `n / 64`
/// for call `n`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calls([u64; WORDS]);

impl Calls {
    /// Every call: the map of an empty chain.
    pub const ALL: Calls = Calls([u64::MAX; WORDS]);
    /// No call.
    pub const NONE: Calls = Calls([0; WORDS]);

    /// Whether call `nr` is in the set; never one past [`CALLS`] or below
    /// 0.
    #[inline]
    pub fn contains(&self, nr: u64) -> bool {
        let Ok(index) = usize::try_from(nr) else { return false };
        index < CALLS && self.0[index / 64] & (1 << (index % 64)) != 0
    }

    /// The calls in both sets.
    pub fn meet(&self, other: &Calls) -> Calls {
        Calls(core::array::from_fn(|i| self.0[i] & other.0[i]))
    }

    /// Its words, which the kernel keeps in atomics for the check of each
    /// call.
    pub fn words(&self) -> [u64; WORDS] {
        self.0
    }

    /// How many calls the set holds.
    pub fn count(&self) -> u32 {
        self.0.iter().map(|word| word.count_ones()).sum()
    }

    fn remove(&mut self, index: usize) {
        self.0[index / 64] &= !(1 << (index % 64));
    }
}

impl<S: AsRef<[Instruction]>> Program<S> {
    /// The calls of `arch` this program allows whatever their arguments
    /// and the instruction pointer.
    pub fn always_allows(&self, arch: u32) -> Calls {
        let mut calls = Calls::ALL;
        for (index, nr) in (0u32..).take(CALLS).enumerate() {
            if !allows_whatever_the_arguments(self.instructions(), nr, arch) {
                calls.remove(index);
            }
        }
        calls
    }
}

/// Whether `program` returns `ALLOW` for call `nr` of `arch` without
/// looking at anything else: Linux's `seccomp_is_const_allow`.
pub(crate) fn allows_whatever_the_arguments(program: &[Instruction], nr: u32, arch: u32) -> bool {
    let mut a = 0u32;
    let mut pc = 0;
    while let Some(&Instruction { code, jt, jf, k }) = program.get(pc) {
        pc += 1;
        match code {
            c if c == LD | W | ABS => {
                a = match k {
                    NR => nr,
                    ARCH => arch,
                    _ => return false,
                };
            }
            c if c == RET | K => return k == crate::seccomp::RET_ALLOW,
            c if c == JMP | JA => pc += k as usize,
            c if c == JMP | JEQ | K => pc += usize::from(if a == k { jt } else { jf }),
            c if c == JMP | JGT | K => pc += usize::from(if a > k { jt } else { jf }),
            c if c == JMP | JGE | K => pc += usize::from(if a >= k { jt } else { jf }),
            c if c == JMP | JSET | K => pc += usize::from(if a & k != 0 { jt } else { jf }),
            c if c == ALU | AND | K => a &= k,
            _ => return false,
        }
    }
    false
}
