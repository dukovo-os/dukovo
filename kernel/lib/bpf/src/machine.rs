// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! McCanne and Jacobson's machine: an accumulator `A`, an index register
//! `X`, sixteen words of memory, and instructions that load from the
//! input, do arithmetic, jump forward and return a value. [`check`] takes
//! a program only if every instruction is one of the machine's, every
//! jump lands inside it, every memory word is one of the sixteen, no
//! division is by a constant 0, no shift by a constant of 32 or more, and
//! the last instruction returns, as Linux's `bpf_check_classic` does
//! (net/core/filter.c); as jumps go only forward, [`run`] ends after at
//! most one pass.
//!
//! What a program reads is an [`Input`]: a frame for a packet capture,
//! the call's `struct seccomp_data` for a seccomp filter. A load past its
//! end returns 0 from the program, and so does a division by an `X` of 0,
//! as on Linux and the BSDs. A shift by `X` takes its low five bits, as
//! Linux's kernel runs the program it converts a classic one to.

/// The most instructions in a program: Linux's `BPF_MAXINSNS`.
pub const MAX_INSTRUCTIONS: usize = 4096;
/// The words of memory a program has: `BPF_MEMWORDS`.
pub const MEMORY: usize = 16;
/// The bytes of an [`Instruction`] in memory.
pub const INSTRUCTION: usize = 8;

/// One instruction, as Linux's `struct sock_filter` and libpcap's
/// `struct bpf_insn` lay it out: the operation, the instructions to skip
/// when a jump's test holds and when it fails, and its constant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Instruction {
    /// What it does: its class, and its size and mode or operation.
    pub code: u16,
    /// The instructions a jump skips when its test holds.
    pub jt: u8,
    /// Those it skips when the test fails.
    pub jf: u8,
    /// Its constant.
    pub k: u32,
}

impl Instruction {
    /// An instruction of `code` and `k`, which jumps nowhere.
    pub const fn new(code: u16, k: u32) -> Instruction {
        Instruction { code, jt: 0, jf: 0, k }
    }

    /// A jump of `code`, to `jt` instructions on when its test holds and
    /// `jf` when it fails.
    pub const fn jump(code: u16, k: u32, jt: u8, jf: u8) -> Instruction {
        Instruction { code, jt, jf, k }
    }

    /// The instruction `bytes` holds, as a program in this machine's
    /// memory lays it out.
    pub fn from_bytes(bytes: [u8; INSTRUCTION]) -> Instruction {
        Instruction {
            code: u16::from_ne_bytes([bytes[0], bytes[1]]),
            jt: bytes[2],
            jf: bytes[3],
            k: u32::from_ne_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        }
    }
}

/// The parts of an instruction's `code`, as linux/bpf_common.h names
/// them: the class (the low three bits), a load's size and mode, an
/// operation, and whether its operand is `k` or `X`.
pub mod opcode {
    /// Load into `A`.
    pub const LD: u16 = 0x00;
    /// Load into `X`.
    pub const LDX: u16 = 0x01;
    /// Store `A` in a memory word.
    pub const ST: u16 = 0x02;
    /// Store `X` in a memory word.
    pub const STX: u16 = 0x03;
    /// Arithmetic on `A`.
    pub const ALU: u16 = 0x04;
    /// A jump.
    pub const JMP: u16 = 0x05;
    /// The end, with the value to return.
    pub const RET: u16 = 0x06;
    /// A move between `A` and `X`.
    pub const MISC: u16 = 0x07;

    /// A load of a 32-bit word.
    pub const W: u16 = 0x00;
    /// A load of a 16-bit half word.
    pub const H: u16 = 0x08;
    /// A load of a byte.
    pub const B: u16 = 0x10;

    /// A load of the constant `k`.
    pub const IMM: u16 = 0x00;
    /// A load of the input at offset `k`.
    pub const ABS: u16 = 0x20;
    /// A load of the input at offset `X + k`.
    pub const IND: u16 = 0x40;
    /// A load of memory word `k`.
    pub const MEM: u16 = 0x60;
    /// A load of the input's length.
    pub const LEN: u16 = 0x80;
    /// `X` = the length of the IPv4 header at `k`, in bytes.
    pub const MSH: u16 = 0xa0;

    /// The operand is `k`; for a RET, the value is.
    pub const K: u16 = 0x00;
    /// The operand is `X`.
    pub const X: u16 = 0x08;
    /// A RET's value is `A`.
    pub const A: u16 = 0x10;

    /// `A + operand`.
    pub const ADD: u16 = 0x00;
    /// `A - operand`.
    pub const SUB: u16 = 0x10;
    /// `A * operand`.
    pub const MUL: u16 = 0x20;
    /// `A / operand`.
    pub const DIV: u16 = 0x30;
    /// `A | operand`.
    pub const OR: u16 = 0x40;
    /// `A & operand`.
    pub const AND: u16 = 0x50;
    /// `A << operand`.
    pub const LSH: u16 = 0x60;
    /// `A >> operand`.
    pub const RSH: u16 = 0x70;
    /// `-A`.
    pub const NEG: u16 = 0x80;
    /// `A % operand`.
    pub const MOD: u16 = 0x90;
    /// `A ^ operand`.
    pub const XOR: u16 = 0xa0;

    /// A jump of `k` instructions.
    pub const JA: u16 = 0x00;
    /// A jump by `jt` when `A == operand`, else by `jf`.
    pub const JEQ: u16 = 0x10;
    /// The same when `A > operand`.
    pub const JGT: u16 = 0x20;
    /// The same when `A >= operand`.
    pub const JGE: u16 = 0x30;
    /// The same when `A & operand` is not 0.
    pub const JSET: u16 = 0x40;

    /// `X = A`.
    pub const TAX: u16 = 0x00;
    /// `A = X`.
    pub const TXA: u16 = 0x80;
}

use opcode::{
    A, ABS, ADD, ALU, AND, B, DIV, H, IMM, IND, JA, JEQ, JGE, JGT, JMP, JSET, K, LD, LDX, LEN, LSH,
    MEM, MISC, MOD, MSH, MUL, NEG, OR, RET, RSH, ST, STX, SUB, TAX, TXA, W, X, XOR,
};

/// Why a program was refused. Linux answers each with EINVAL; the
/// place is for the log and the tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// It has no instructions, or more than [`MAX_INSTRUCTIONS`].
    Length,
    /// Its instruction at this place is not one of the machine's, or is
    /// one that can never work: a division by a constant 0, a shift by a
    /// constant of 32 or more, a memory word past the sixteen. For a
    /// seccomp filter, also one seccomp does not allow.
    Instruction(usize),
    /// Its jump at this place lands past its end.
    Jump(usize),
    /// Its load of a memory word at this place may come before any store
    /// of that word: a seccomp filter's check alone refuses it.
    Unset(usize),
    /// Its last instruction does not return.
    NoReturn,
}

/// A set of instruction codes, made from a list as the crate compiles:
/// whether it holds a code is one look rather than a search, so the Kani
/// proofs' unwind bound need only cover the steps of a run.
pub(crate) struct Codes([bool; 256]);

impl Codes {
    /// The set of `codes`, each below 256, as every classic BPF code is.
    pub(crate) const fn of(codes: &[u16]) -> Codes {
        let mut set = [false; 256];
        let mut at = 0;
        while at < codes.len() {
            set[codes[at] as usize] = true;
            at += 1;
        }
        Codes(set)
    }

    /// Whether `code` is one of the set's.
    pub(crate) fn has(&self, code: u16) -> bool {
        self.0.get(usize::from(code)).copied().unwrap_or(false)
    }
}

/// The instructions that are always ones the machine runs.
const PLAIN: Codes = Codes::of(&[
    LD | W | ABS,
    LD | H | ABS,
    LD | B | ABS,
    LD | W | IND,
    LD | H | IND,
    LD | B | IND,
    LD | W | LEN,
    LD | W | IMM,
    LDX | W | IMM,
    LDX | W | LEN,
    LDX | B | MSH,
    ALU | NEG,
    RET | K,
    RET | A,
    MISC | TAX,
    MISC | TXA,
]);
/// Those that load or store a memory word, their constant's.
const WITH_MEMORY: Codes = Codes::of(&[LD | W | MEM, LDX | W | MEM, ST, STX]);
/// The arithmetic operations besides NEG, on the constant or on `X`.
const ARITHMETIC: Codes = Codes::of(&[ADD, SUB, MUL, DIV, OR, AND, LSH, RSH, MOD, XOR]);
/// The jumps that test `A`, against the constant or `X`.
const TESTS: Codes = Codes::of(&[JEQ, JGT, JGE, JSET]);

/// Whether `code` is one of the jumps that test `A`.
pub(crate) fn is_test(code: u16) -> bool {
    code & !0xf8 == JMP && TESTS.has(code & 0xf0)
}

/// Whether `insn`, at `at` in a program of `len` instructions, is one
/// the machine runs.
fn valid(insn: Instruction, at: usize, len: usize) -> Result<(), Invalid> {
    let code = insn.code;
    // Whether a jump of `skip` lands on one of the instructions after.
    let lands = |skip: u32| (skip as usize) < len - at - 1;
    let ok = if PLAIN.has(code) {
        true
    } else if WITH_MEMORY.has(code) {
        (insn.k as usize) < MEMORY
    } else if code & !0xf8 == ALU && ARITHMETIC.has(code & 0xf0) {
        // A constant divisor of 0, or shift by 32 or more, never works.
        code & X == X
            || match code & 0xf0 {
                DIV | MOD => insn.k != 0,
                LSH | RSH => insn.k < 32,
                _ => true,
            }
    } else if code == JMP | JA {
        return if lands(insn.k) { Ok(()) } else { Err(Invalid::Jump(at)) };
    } else if is_test(code) {
        return if lands(u32::from(insn.jt)) && lands(u32::from(insn.jf)) {
            Ok(())
        } else {
            Err(Invalid::Jump(at))
        };
    } else {
        false
    };
    if ok { Ok(()) } else { Err(Invalid::Instruction(at)) }
}

/// Whether `program` is one the machine runs: what [`run`] needs of it.
///
/// # Errors
///
/// Why it is not: the first fault, in the program's order.
pub fn check(program: &[Instruction]) -> Result<(), Invalid> {
    if program.is_empty() || program.len() > MAX_INSTRUCTIONS {
        return Err(Invalid::Length);
    }
    for (at, &insn) in program.iter().enumerate() {
        valid(insn, at, program.len())?;
    }
    match program.last() {
        Some(last) if last.code & 0x07 == RET => Ok(()),
        _ => Err(Invalid::NoReturn),
    }
}

/// What a program reads.
pub trait Input {
    /// Its length in bytes, which `LEN` loads.
    fn length(&self) -> u32;

    /// The `size` bytes (1, 2 or 4) at `offset`, as a number, if they
    /// are all there and are a load this input has.
    fn load(&self, offset: u64, size: usize) -> Option<u32>;
}

/// The bytes a load of `code` takes.
fn size(code: u16) -> usize {
    match code & 0x18 {
        H => 2,
        B => 1,
        _ => 4,
    }
}

/// Runs `program`, which [`check`] took, on `input`: the value of the RET
/// it reaches, or 0 when it loads past the input's end or divides by an
/// `X` of 0.
pub fn run<I: Input + ?Sized>(program: &[Instruction], input: &I) -> u32 {
    let (mut a, mut x) = (0u32, 0u32);
    let mut memory = [0u32; MEMORY];
    let mut pc = 0;
    while let Some(&insn) = program.get(pc) {
        pc += 1;
        let k = insn.k;
        match insn.code & 0x07 {
            LD => {
                a = match insn.code & 0xe0 {
                    ABS => match input.load(u64::from(k), size(insn.code)) {
                        Some(word) => word,
                        None => return 0,
                    },
                    IND => match input.load(u64::from(x) + u64::from(k), size(insn.code)) {
                        Some(word) => word,
                        None => return 0,
                    },
                    LEN => input.length(),
                    MEM => memory[k as usize % MEMORY],
                    _ => k,
                }
            }
            LDX => {
                x = match insn.code & 0xe0 {
                    LEN => input.length(),
                    MEM => memory[k as usize % MEMORY],
                    // The length of the IPv4 header at `k`, in bytes.
                    MSH => match input.load(u64::from(k), 1) {
                        Some(byte) => (byte & 0xf) * 4,
                        None => return 0,
                    },
                    _ => k,
                }
            }
            ST => memory[k as usize % MEMORY] = a,
            STX => memory[k as usize % MEMORY] = x,
            ALU => {
                let operand = if insn.code & 0x08 == X { x } else { k };
                a = match insn.code & 0xf0 {
                    ADD => a.wrapping_add(operand),
                    SUB => a.wrapping_sub(operand),
                    MUL => a.wrapping_mul(operand),
                    DIV => match a.checked_div(operand) {
                        Some(a) => a,
                        None => return 0,
                    },
                    MOD => match a.checked_rem(operand) {
                        Some(a) => a,
                        None => return 0,
                    },
                    OR => a | operand,
                    AND => a & operand,
                    XOR => a ^ operand,
                    LSH => a << (operand & 31),
                    RSH => a >> (operand & 31),
                    _ => a.wrapping_neg(),
                }
            }
            JMP => {
                let operand = if insn.code & 0x08 == X { x } else { k };
                let holds = match insn.code & 0xf0 {
                    JA => {
                        pc += k as usize;
                        continue;
                    }
                    JEQ => a == operand,
                    JGT => a > operand,
                    JGE => a >= operand,
                    _ => a & operand != 0,
                };
                pc += usize::from(if holds { insn.jt } else { insn.jf });
            }
            RET => return if insn.code & 0x18 == A { a } else { k },
            _ => {
                if insn.code & 0xf8 == TXA {
                    a = x;
                } else {
                    x = a;
                }
            }
        }
    }
    0
}
