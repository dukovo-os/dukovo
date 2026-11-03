// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Classic BPF, the packet filters libpcap compiles from what tcpdump(8)
//! is asked for (`udp port 53`), which the kernel runs on each frame a
//! packet capture sees (ADR 0346), as Linux runs those SO_ATTACH_FILTER
//! attaches and the BSDs those BIOCSETF sets, so that a frame the filter
//! does not want is never copied.
//!
//! A program is McCanne and Jacobson's machine: an accumulator `A`, an
//! index register `X`, sixteen words of memory, and instructions that
//! load from the frame, do arithmetic, jump forward and return how many
//! of the frame's bytes to keep, 0 for none. [`check`] takes a program
//! only if every instruction is one of the machine's, every jump lands
//! inside it, every memory word is one of the sixteen, no division is by
//! a constant 0 and the last instruction returns, as Linux's
//! `bpf_check_classic` does; as jumps go only forward, [`run`] ends
//! after at most one pass. A load past the frame's end returns 0, and so
//! does a division by an `X` of 0, as on Linux and the BSDs; a shift by
//! 32 or more leaves 0, as libpcap's own interpreter has it. Linux's
//! extensions, at the negative offsets of SKF_AD_OFF, are not loads this
//! machine has: libpcap compiles them only for Linux.

/// The most instructions in a program: Linux's BPF_MAXINSNS.
pub const MAX_INSTRUCTIONS: usize = 4096;
/// The words of memory a program has: BPF_MEMWORDS.
pub const MEMORY: usize = 16;

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

/// The bytes of an [`Instruction`] in memory.
pub const INSTRUCTION: usize = 8;

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

// The instruction classes.
const LD: u16 = 0x00;
const LDX: u16 = 0x01;
const ST: u16 = 0x02;
const STX: u16 = 0x03;
const ALU: u16 = 0x04;
const JMP: u16 = 0x05;
const RET: u16 = 0x06;
const MISC: u16 = 0x07;

// A load's size.
const W: u16 = 0x00;
const H: u16 = 0x08;
const B: u16 = 0x10;

// A load's mode.
const IMM: u16 = 0x00;
const ABS: u16 = 0x20;
const IND: u16 = 0x40;
const MEM: u16 = 0x60;
const LEN: u16 = 0x80;
const MSH: u16 = 0xa0;

// What an arithmetic or jump instruction takes: its constant, or `X`;
// and what a return returns: its constant, or `A`.
const K: u16 = 0x00;
const X: u16 = 0x08;
const A: u16 = 0x10;

// The arithmetic operations.
const ADD: u16 = 0x00;
const SUB: u16 = 0x10;
const MUL: u16 = 0x20;
const DIV: u16 = 0x30;
const OR: u16 = 0x40;
const AND: u16 = 0x50;
const LSH: u16 = 0x60;
const RSH: u16 = 0x70;
const NEG: u16 = 0x80;
const MOD: u16 = 0x90;
const XOR: u16 = 0xa0;

// The jumps.
const JA: u16 = 0x00;
const JEQ: u16 = 0x10;
const JGT: u16 = 0x20;
const JGE: u16 = 0x30;
const JSET: u16 = 0x40;

// MISC's two: `X = A` and `A = X`.
const TAX: u16 = 0x00;
const TXA: u16 = 0x80;

/// Why [`check`] refused a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// It has no instructions, or more than [`MAX_INSTRUCTIONS`].
    Length,
    /// Its instruction at this place is not one of the machine's, or is
    /// one that can never work: a division by a constant 0, a memory word
    /// past the sixteen.
    Instruction(usize),
    /// Its jump at this place lands past its end.
    Jump(usize),
    /// Its last instruction does not return.
    NoReturn,
}

/// The instructions that are always ones the machine runs.
const PLAIN: [u16; 16] = [
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
];
/// Those that load or store a memory word, their constant's.
const WITH_MEMORY: [u16; 4] = [LD | W | MEM, LDX | W | MEM, ST, STX];
/// The arithmetic operations besides NEG, on the constant or on `X`.
const ARITHMETIC: [u16; 10] = [ADD, SUB, MUL, DIV, OR, AND, LSH, RSH, MOD, XOR];
/// The jumps that test `A`, against the constant or `X`.
const TESTS: [u16; 4] = [JEQ, JGT, JGE, JSET];

/// Whether `insn`, at `at` in a program of `len` instructions, is one
/// the machine runs.
fn valid(insn: Instruction, at: usize, len: usize) -> Result<(), Invalid> {
    let code = insn.code;
    // Whether a jump of `skip` lands on one of the instructions after.
    let lands = |skip: u32| (skip as usize) < len - at - 1;
    let ok = if PLAIN.contains(&code) {
        true
    } else if WITH_MEMORY.contains(&code) {
        (insn.k as usize) < MEMORY
    } else if code & !0xf8 == ALU && ARITHMETIC.contains(&(code & 0xf0)) {
        // A constant divisor of 0, or shift by 32 or more, never works.
        code & X == X
            || match code & 0xf0 {
                DIV | MOD => insn.k != 0,
                LSH | RSH => insn.k < 32,
                _ => true,
            }
    } else if code == JMP | JA {
        return if lands(insn.k) { Ok(()) } else { Err(Invalid::Jump(at)) };
    } else if code & !0xf8 == JMP && TESTS.contains(&(code & 0xf0)) {
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
/// Why it is not.
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

/// A frame as a program sees it: `head`, bytes the kernel puts before
/// it (the Ethernet header it gives a loopback packet), then `body`, the
/// frame itself.
#[derive(Clone, Copy, Debug)]
pub struct Packet<'a> {
    /// What comes before the frame.
    pub head: &'a [u8],
    /// The frame.
    pub body: &'a [u8],
}

impl Packet<'_> {
    /// Its length.
    pub fn len(&self) -> usize {
        self.head.len() + self.body.len()
    }

    /// Whether it has no bytes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn byte(&self, at: usize) -> u8 {
        match at.checked_sub(self.head.len()) {
            None => self.head[at],
            Some(at) => self.body[at],
        }
    }

    /// The `size` bytes at `offset`, big-endian, if they are all there.
    fn load(&self, offset: u64, size: usize) -> Option<u32> {
        let at = usize::try_from(offset).ok()?;
        if at.checked_add(size)? > self.len() {
            return None;
        }
        Some((at..at + size).fold(0, |word, i| (word << 8) | u32::from(self.byte(i))))
    }
}

/// The bytes a load of `code` takes.
fn size(code: u16) -> usize {
    match code & 0x18 {
        H => 2,
        B => 1,
        _ => 4,
    }
}

/// Runs `program`, which [`check`] took, on `packet`: how many of its
/// bytes to keep, 0 for none.
pub fn run(program: &[Instruction], packet: Packet<'_>) -> u32 {
    let (mut a, mut x) = (0u32, 0u32);
    let mut memory = [0u32; MEMORY];
    // A frame is shorter than 4 GiB.
    let len = packet.len() as u32;
    let mut pc = 0;
    while let Some(&insn) = program.get(pc) {
        pc += 1;
        let k = insn.k;
        match insn.code & 0x07 {
            LD => {
                a = match insn.code & 0xe0 {
                    ABS => match packet.load(u64::from(k), size(insn.code)) {
                        Some(word) => word,
                        None => return 0,
                    },
                    IND => match packet.load(u64::from(x) + u64::from(k), size(insn.code)) {
                        Some(word) => word,
                        None => return 0,
                    },
                    LEN => len,
                    MEM => memory[k as usize % MEMORY],
                    _ => k,
                }
            }
            LDX => {
                x = match insn.code & 0xe0 {
                    LEN => len,
                    MEM => memory[k as usize % MEMORY],
                    // The length of the IPv4 header at `k`, in bytes.
                    MSH => match packet.load(u64::from(k), 1) {
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
                    LSH => a.checked_shl(operand).unwrap_or(0),
                    RSH => a.checked_shr(operand).unwrap_or(0),
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

#[cfg(test)]
mod tests {
    use super::*;

    const fn i(code: u16, k: u32) -> Instruction {
        Instruction::new(code, k)
    }

    const fn j(code: u16, k: u32, jt: u8, jf: u8) -> Instruction {
        Instruction::jump(code, k, jt, jf)
    }

    /// `tcpdump -dd udp port 53`, as libpcap 1.10 compiles it for an
    /// Ethernet link.
    const UDP_53: [Instruction; 18] = [
        i(0x28, 12),
        j(0x15, 0x86dd, 0, 6),
        i(0x30, 20),
        j(0x15, 0x11, 0, 15),
        i(0x28, 54),
        j(0x15, 53, 12, 0),
        i(0x28, 56),
        j(0x15, 53, 10, 11),
        j(0x15, 0x0800, 0, 10),
        i(0x30, 23),
        j(0x15, 0x11, 0, 8),
        i(0x28, 20),
        j(0x45, 0x1fff, 6, 0),
        i(0xb1, 14),
        i(0x48, 14),
        j(0x15, 53, 2, 0),
        i(0x48, 16),
        j(0x15, 53, 0, 1),
    ];

    /// UDP_53, with its two returns.
    fn udp_53() -> [Instruction; 20] {
        let mut program = [i(0x06, 0); 20];
        program[..18].copy_from_slice(&UDP_53);
        program[18] = i(0x06, 262_144);
        program[19] = i(0x06, 0);
        program
    }

    /// An Ethernet frame holding an IPv4 UDP datagram from port `from` to
    /// port `to`, with `options` words of IP options.
    fn udp_frame(from: u16, to: u16, options: usize) -> ([u8; 64], usize) {
        let mut frame = [0u8; 64];
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        frame[14] = 0x45 + options as u8;
        frame[23] = 17;
        let udp = 14 + 20 + 4 * options;
        frame[udp..udp + 2].copy_from_slice(&from.to_be_bytes());
        frame[udp + 2..udp + 4].copy_from_slice(&to.to_be_bytes());
        (frame, udp + 8)
    }

    fn whole(frame: &[u8]) -> Packet<'_> {
        Packet { head: &[], body: frame }
    }

    #[test]
    fn libpcaps_program_finds_the_port() {
        let program = udp_53();
        check(&program).unwrap();
        let (frame, len) = udp_frame(40000, 53, 0);
        assert_eq!(run(&program, whole(&frame[..len])), 262_144);
        let (frame, len) = udp_frame(53, 40000, 0);
        assert_eq!(run(&program, whole(&frame[..len])), 262_144);
        let (frame, len) = udp_frame(40000, 80, 0);
        assert_eq!(run(&program, whole(&frame[..len])), 0);
        // Past the options, by the header's length (MSH).
        let (frame, len) = udp_frame(40000, 53, 2);
        assert_eq!(run(&program, whole(&frame[..len])), 262_144);
        // A frame cut before its ports is not one it wants.
        let (frame, _) = udp_frame(40000, 53, 0);
        assert_eq!(run(&program, whole(&frame[..36])), 0);
    }

    #[test]
    fn a_head_comes_before_the_frame() {
        let program = [i(0x20, 0), j(0x15, 0x0102_0304, 0, 1), i(0x06, 9), i(0x06, 0)];
        check(&program).unwrap();
        // The word spans the head and the frame.
        let packet = Packet { head: &[1, 2], body: &[3, 4] };
        assert_eq!(run(&program, packet), 9);
        let packet = Packet { head: &[1, 2, 3], body: &[5] };
        assert_eq!(run(&program, packet), 0);
    }

    #[test]
    fn loads_past_the_end_return_nothing() {
        for code in [0x20, 0x28, 0x30] {
            let program = [i(code, 3), i(0x06, 1)];
            assert_eq!(run(&program, whole(&[0; 3])), 0, "{code:#x}");
        }
        // X + k past the end, or past 4 GiB.
        let program = [i(0x01, u32::MAX), i(0x50, 2), i(0x06, 1)];
        check(&program).unwrap();
        assert_eq!(run(&program, whole(&[0; 8])), 0);
        let program = [i(0xb1, 8), i(0x06, 1)];
        assert_eq!(run(&program, whole(&[0; 8])), 0);
    }

    #[test]
    fn arithmetic_is_the_machines() {
        // A = len; A *= 3; A -= 1; A %= 7; return A.
        let program = [i(0x80, 0), i(0x24, 3), i(0x14, 1), i(0x94, 7), i(0x16, 0)];
        check(&program).unwrap();
        assert_eq!(run(&program, whole(&[0; 10])), (10 * 3 - 1) % 7);
        // A division by an X of 0 returns 0.
        let program = [i(0x00, 5), i(0x01, 0), i(0x3c, 0), i(0x06, 1)];
        assert_eq!(run(&program, whole(&[])), 0);
        // A shift by an X of 32 leaves 0.
        let program = [i(0x00, 1), i(0x01, 32), i(0x6c, 0), i(0x16, 0)];
        assert_eq!(run(&program, whole(&[])), 0);
        // Memory, TAX and TXA, NEG.
        let program = [
            i(0x00, 6),
            i(0x02, 15),
            i(0x00, 0),
            i(0x61, 15),
            i(0x87, 0),
            i(0x84, 0),
            i(0x07, 0),
            i(0x87, 0),
            i(0x16, 0),
        ];
        check(&program).unwrap();
        assert_eq!(run(&program, whole(&[])), 6u32.wrapping_neg());
    }

    #[test]
    fn jumps_go_forward() {
        // JSET, JGT, JGE, JA.
        let program = [
            i(0x00, 0b1010),
            j(0x45, 0b0010, 1, 0),
            i(0x06, 1),
            j(0x25, 9, 1, 0),
            i(0x06, 2),
            j(0x35, 11, 0, 1),
            i(0x06, 3),
            i(0x05, 1),
            i(0x06, 4),
            i(0x06, 5),
        ];
        check(&program).unwrap();
        assert_eq!(run(&program, whole(&[])), 5);
    }

    #[test]
    fn check_refuses_what_cannot_run() {
        assert_eq!(check(&[]), Err(Invalid::Length));
        assert_eq!(check(&vec![i(0x06, 0); MAX_INSTRUCTIONS + 1]), Err(Invalid::Length));
        check(&vec![i(0x06, 0); MAX_INSTRUCTIONS]).unwrap();
        // Off the end.
        assert_eq!(check(&[i(0x05, 1), i(0x06, 0)]), Err(Invalid::Jump(0)));
        assert_eq!(check(&[j(0x15, 0, 0, 1), i(0x06, 0)]), Err(Invalid::Jump(0)));
        assert_eq!(check(&[j(0x15, 0, 1, 0), i(0x06, 0)]), Err(Invalid::Jump(0)));
        // Memory past the sixteen words, a constant 0 divisor, a shift by
        // 32, an operation that is not one.
        assert_eq!(check(&[i(0x02, 16), i(0x06, 0)]), Err(Invalid::Instruction(0)));
        assert_eq!(check(&[i(0x61, 16), i(0x06, 0)]), Err(Invalid::Instruction(0)));
        assert_eq!(check(&[i(0x34, 0), i(0x06, 0)]), Err(Invalid::Instruction(0)));
        assert_eq!(check(&[i(0x94, 0), i(0x06, 0)]), Err(Invalid::Instruction(0)));
        assert_eq!(check(&[i(0x64, 32), i(0x06, 0)]), Err(Invalid::Instruction(0)));
        assert_eq!(check(&[i(0xb4, 0), i(0x06, 0)]), Err(Invalid::Instruction(0)));
        assert_eq!(check(&[j(0xc5, 0, 0, 0), i(0x06, 0)]), Err(Invalid::Instruction(0)));
        // RET X is not one, nor a load of H by LEN.
        assert_eq!(check(&[i(0x0e, 0)]), Err(Invalid::Instruction(0)));
        assert_eq!(check(&[i(0x88, 0), i(0x06, 0)]), Err(Invalid::Instruction(0)));
        // The last must return.
        assert_eq!(check(&[i(0x06, 0), i(0x00, 0)]), Err(Invalid::NoReturn));
    }

    #[test]
    fn instructions_read_as_laid_out() {
        let insn = j(0x15, 0x86dd, 3, 7);
        let mut bytes = [0u8; INSTRUCTION];
        bytes[..2].copy_from_slice(&insn.code.to_ne_bytes());
        bytes[2] = 3;
        bytes[3] = 7;
        bytes[4..].copy_from_slice(&insn.k.to_ne_bytes());
        assert_eq!(Instruction::from_bytes(bytes), insn);
    }
}
