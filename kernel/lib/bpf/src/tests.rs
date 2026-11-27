// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The machine's checks and interpreter; a seccomp filter's checks, run
//! and cache against Linux's rules, and against two filters libseccomp
//! wrote (`fixtures/make.py`): Podman's default profile and a small allow
//! list. radix-net's tests run libpcap's packet filters.

use crate::calls::{CALLS, Calls};
use crate::filter::Program;
use crate::machine::opcode::{
    A, ABS, ADD, ALU, AND, B, DIV, H, IMM, IND, JA, JEQ, JGE, JGT, JMP, JSET, K, LD, LDX, LEN, LSH,
    MEM, MISC, MOD, MUL, NEG, OR, RET, RSH, ST, STX, SUB, TAX, TXA, W, X, XOR,
};
use crate::machine::{self, INSTRUCTION, Input, Instruction, Invalid, MAX_INSTRUCTIONS};
use crate::seccomp::{
    AUDIT_ARCH_X86_64, Action, RET_ALLOW, RET_ERRNO, RET_KILL_PROCESS, RET_KILL_THREAD, RET_LOG,
    RET_TRAP, SeccompData, stricter,
};

/// x86-64's numbers of the calls the tests make.
const READ: i32 = 0;
const IOCTL: i32 = 16;
const SOCKET: i32 = 41;
const GETPPID: i32 = 110;
const PERSONALITY: i32 = 135;
const BDFLUSH: i32 = 134;

/// The arch of a 32-bit x86 call (`AUDIT_ARCH_I386`).
const ARCH_X86: u32 = 0x4000_0003;

const TCGETS: u64 = 0x5401;
const EPERM: u32 = 1;
const EACCES: u32 = 13;
const EINVAL: u32 = 22;
const ENOSYS: u32 = 38;

fn i(code: u16, jt: u8, jf: u8, k: u32) -> Instruction {
    Instruction::jump(code, k, jt, jf)
}

fn check(program: &[Instruction]) -> Result<Program<Vec<Instruction>>, Invalid> {
    Program::check(program.to_vec(), &mut [0; MAX_INSTRUCTIONS])
}

fn checked(program: &[Instruction]) -> Program<Vec<Instruction>> {
    check(program).unwrap()
}

/// A program a fixture holds.
fn fixture(bytes: &[u8]) -> Program<Vec<Instruction>> {
    let (chunks, rest) = bytes.as_chunks::<INSTRUCTION>();
    assert!(rest.is_empty(), "a fixture holds whole instructions");
    let instructions: Vec<Instruction> =
        chunks.iter().map(|chunk| Instruction::from_bytes(*chunk)).collect();
    Program::check(instructions, &mut [0; MAX_INSTRUCTIONS]).unwrap()
}

fn call(nr: i32, args: [u64; 6]) -> SeccompData {
    SeccompData { nr, arch: AUDIT_ARCH_X86_64, instruction_pointer: 0x40_1000, args }
}

/// The program `ld [k]; ret a`: the data's word at offset k.
fn word_at(k: u32) -> Program<Vec<Instruction>> {
    checked(&[i(LD | W | ABS, 0, 0, k), i(RET | A, 0, 0, 0)])
}

#[test]
fn the_data_is_read_as_x86_64_lays_it_out() {
    let data = SeccompData {
        nr: -1,
        arch: AUDIT_ARCH_X86_64,
        instruction_pointer: 0x1122_3344_5566_7788,
        args: [0xa0a1_a2a3_a4a5_a6a7, 2, 3, 4, 5, 0xf0f1_f2f3_f4f5_f6f7],
    };
    assert_eq!(word_at(0).run(&data), u32::MAX);
    assert_eq!(word_at(4).run(&data), AUDIT_ARCH_X86_64);
    assert_eq!(word_at(8).run(&data), 0x5566_7788);
    assert_eq!(word_at(12).run(&data), 0x1122_3344);
    assert_eq!(word_at(16).run(&data), 0xa4a5_a6a7);
    assert_eq!(word_at(20).run(&data), 0xa0a1_a2a3);
    assert_eq!(word_at(56).run(&data), 0xf4f5_f6f7);
    assert_eq!(word_at(60).run(&data), 0xf0f1_f2f3);
}

#[test]
fn a_program_of_no_instruction_or_too_many_is_refused() {
    assert_eq!(check(&[]).unwrap_err(), Invalid::Length);
    let long = vec![i(RET | K, 0, 0, RET_ALLOW); MAX_INSTRUCTIONS + 1];
    assert_eq!(check(&long).unwrap_err(), Invalid::Length);
    assert!(check(&long[..MAX_INSTRUCTIONS]).is_ok());
    let mut short_masks = [0; 1];
    let two = vec![i(RET | K, 0, 0, RET_ALLOW); 2];
    assert_eq!(Program::check(two, &mut short_masks).unwrap_err(), Invalid::Length);
}

#[test]
fn loads_seccomp_does_not_allow_are_refused() {
    let ret = i(RET | K, 0, 0, RET_ALLOW);
    for code in [LD | H | ABS, LD | B | ABS, LD | W | IND, LDX | B | 0xa0, MISC | 0x40] {
        assert_eq!(
            check(&[i(code, 0, 0, 0), ret]).unwrap_err(),
            Invalid::Instruction(0),
            "{code:#x}"
        );
    }
    // A word outside the 64 bytes, or not on a word's boundary.
    assert_eq!(check(&[i(LD | W | ABS, 0, 0, 64), ret]).unwrap_err(), Invalid::Instruction(0));
    assert_eq!(check(&[i(LD | W | ABS, 0, 0, 2), ret]).unwrap_err(), Invalid::Instruction(0));
    assert!(check(&[i(LD | W | ABS, 0, 0, 60), ret]).is_ok());
}

#[test]
fn mod_is_refused_though_the_machine_has_it() {
    let ret = i(RET | K, 0, 0, RET_ALLOW);
    let program = [i(ALU | MOD | K, 0, 0, 3), ret];
    assert_eq!(machine::check(&program), Ok(()));
    assert_eq!(check(&program).unwrap_err(), Invalid::Instruction(0));
    assert_eq!(check(&[i(ALU | MOD | X, 0, 0, 0), ret]).unwrap_err(), Invalid::Instruction(0));
}

#[test]
fn constants_out_of_range_are_refused() {
    let ret = i(RET | K, 0, 0, RET_ALLOW);
    assert_eq!(check(&[i(ALU | DIV | K, 0, 0, 0), ret]).unwrap_err(), Invalid::Instruction(0));
    assert_eq!(check(&[i(ALU | LSH | K, 0, 0, 32), ret]).unwrap_err(), Invalid::Instruction(0));
    assert_eq!(check(&[i(ALU | RSH | K, 0, 0, 40), ret]).unwrap_err(), Invalid::Instruction(0));
    assert!(check(&[i(ALU | LSH | K, 0, 0, 31), ret]).is_ok());
    assert_eq!(check(&[i(ST, 0, 0, 16), ret]).unwrap_err(), Invalid::Instruction(0));
    assert_eq!(check(&[i(LDX | MEM, 0, 0, 16), ret]).unwrap_err(), Invalid::Instruction(0));
}

#[test]
fn a_jump_past_the_end_is_refused() {
    let ret = i(RET | K, 0, 0, RET_ALLOW);
    assert_eq!(check(&[i(JMP | JA, 0, 0, 1), ret]).unwrap_err(), Invalid::Jump(0));
    assert!(check(&[i(JMP | JA, 0, 0, 0), ret]).is_ok());
    assert_eq!(check(&[i(JMP | JEQ | K, 1, 0, 0), ret]).unwrap_err(), Invalid::Jump(0));
    assert_eq!(check(&[i(JMP | JGT | X, 0, 1, 0), ret]).unwrap_err(), Invalid::Jump(0));
    // A jump of every length the 8 bits allow, into a long program.
    let mut long = vec![ret; 300];
    long[0] = i(JMP | JSET | K, 255, 254, 1);
    assert!(check(&long).is_ok());
    // A JA of 2^32 - 1 must not wrap round into the program.
    assert_eq!(check(&[i(JMP | JA, 0, 0, u32::MAX), ret]).unwrap_err(), Invalid::Jump(0));
}

#[test]
fn a_program_must_end_with_a_return() {
    assert_eq!(check(&[i(LD | IMM, 0, 0, 1)]).unwrap_err(), Invalid::NoReturn);
    assert_eq!(check(&[i(RET | X, 0, 0, 0)]).unwrap_err(), Invalid::Instruction(0));
    assert!(check(&[i(RET | A, 0, 0, 0)]).is_ok());
}

#[test]
fn a_scratch_word_is_read_only_after_every_path_stored_it() {
    let ret = i(RET | A, 0, 0, 0);
    assert_eq!(check(&[i(LD | MEM, 0, 0, 3), ret]).unwrap_err(), Invalid::Unset(0));
    assert!(check(&[i(ST, 0, 0, 3), i(LD | MEM, 0, 0, 3), ret]).is_ok());
    // Stored on the path that falls through, not on the one that jumps.
    let one_path = [i(JMP | JEQ | K, 1, 0, 7), i(STX, 0, 0, 5), i(LDX | MEM, 0, 0, 5), ret];
    assert_eq!(check(&one_path).unwrap_err(), Invalid::Unset(2));
    let both_paths =
        [i(ST, 0, 0, 5), i(JMP | JEQ | K, 1, 0, 7), i(STX, 0, 0, 5), i(LDX | MEM, 0, 0, 5), ret];
    assert!(check(&both_paths).is_ok());
}

#[test]
fn the_data_is_64_bytes_long() {
    let program =
        checked(&[i(LDX | W | LEN, 0, 0, 0), i(MISC | TXA, 0, 0, 0), i(RET | A, 0, 0, 0)]);
    assert_eq!(program.run(&SeccompData::default()), 64);
    let program = checked(&[i(LD | W | LEN, 0, 0, 0), i(RET | A, 0, 0, 0)]);
    assert_eq!(program.run(&SeccompData::default()), 64);
}

/// `A = 100; X = operand; A op= X or k; ret A`.
fn alu(op: u16, operand: u32) -> (u32, u32) {
    let by_x = checked(&[
        i(LD | IMM, 0, 0, 100),
        i(LDX | IMM, 0, 0, operand),
        i(ALU | op | X, 0, 0, 0),
        i(RET | A, 0, 0, 0),
    ]);
    let by_k =
        check(&[i(LD | IMM, 0, 0, 100), i(ALU | op | K, 0, 0, operand), i(RET | A, 0, 0, 0)]);
    let data = SeccompData::default();
    (by_x.run(&data), by_k.map_or(u32::MAX, |program| program.run(&data)))
}

#[test]
fn arithmetic_is_32_bits_and_wraps() {
    assert_eq!(alu(ADD, u32::MAX), (99, 99));
    assert_eq!(alu(SUB, 101), (u32::MAX, u32::MAX));
    assert_eq!(alu(MUL, 0x8000_0000), (0, 0));
    assert_eq!(alu(DIV, 7), (14, 14));
    assert_eq!(alu(AND, 0x24), (0x24, 0x24));
    assert_eq!(alu(OR, 1), (101, 101));
    assert_eq!(alu(XOR, 100), (0, 0));
    assert_eq!(alu(LSH, 2), (400, 400));
    assert_eq!(alu(RSH, 2), (25, 25));
    let neg = checked(&[i(LD | IMM, 0, 0, 1), i(ALU | NEG, 0, 0, 0), i(RET | A, 0, 0, 0)]);
    assert_eq!(neg.run(&SeccompData::default()), u32::MAX);
}

#[test]
fn a_division_by_an_x_of_0_returns_0() {
    // Linux's converted program returns 0, KILL_THREAD, at once.
    assert_eq!(alu(DIV, 0).0, RET_KILL_THREAD);
}

#[test]
fn a_shift_by_x_takes_its_low_five_bits() {
    assert_eq!(alu(LSH, 33).0, 200);
    assert_eq!(alu(RSH, 32).0, 100);
}

#[test]
fn jumps_compare_unsigned_with_k_or_x() {
    // A = arg0's low word; returns 1 when the jump holds, 2 when not.
    let program = |code: u16, k: u32| {
        checked(&[
            i(LD | W | ABS, 0, 0, 16),
            i(LDX | IMM, 0, 0, k),
            i(code, 0, 1, k),
            i(RET | K, 0, 0, 1),
            i(RET | K, 0, 0, 2),
        ])
    };
    let with = |arg: u64| call(READ, [arg, 0, 0, 0, 0, 0]);
    for (code, k, arg, holds) in [
        (JMP | JEQ | K, 5, 5, true),
        (JMP | JEQ | X, 5, 6, false),
        (JMP | JGT | K, 5, 6, true),
        (JMP | JGT | K, 5, 5, false),
        (JMP | JGE | X, 5, 5, true),
        (JMP | JGT | K, 1, 0xffff_ffff, true),
        (JMP | JSET | K, 4, 6, true),
        (JMP | JSET | X, 4, 3, false),
    ] {
        let expected = if holds { 1 } else { 2 };
        assert_eq!(program(code, k).run(&with(arg)), expected, "{code:#x} {k} {arg}");
    }
}

#[test]
fn moves_and_scratch_words_carry_values() {
    let program = checked(&[
        i(LD | IMM, 0, 0, 7),
        i(MISC | TAX, 0, 0, 0),
        i(LD | IMM, 0, 0, 9),
        i(ST, 0, 0, 15),
        i(MISC | TXA, 0, 0, 0),
        i(LDX | MEM, 0, 0, 15),
        i(ALU | ADD | X, 0, 0, 0),
        i(RET | A, 0, 0, 0),
    ]);
    assert_eq!(program.run(&SeccompData::default()), 16);
}

#[test]
fn the_strictest_action_of_a_chain_wins_and_ties_keep_the_newest() {
    assert_eq!(stricter(RET_ALLOW, RET_KILL_PROCESS), RET_KILL_PROCESS);
    assert_eq!(stricter(RET_KILL_PROCESS, RET_KILL_THREAD), RET_KILL_PROCESS);
    assert_eq!(stricter(RET_ALLOW, RET_ERRNO | 5), RET_ERRNO | 5);
    assert_eq!(stricter(RET_ERRNO | 5, RET_ERRNO | 9), RET_ERRNO | 5);
    assert_eq!(stricter(RET_ERRNO | 5, RET_TRAP), RET_TRAP);
    assert_eq!(stricter(RET_LOG, RET_ALLOW), RET_LOG);
}

#[test]
fn kills_and_log_are_logged_and_the_rest_only_when_the_filter_asked() {
    for action in [Action::KillProcess, Action::KillThread, Action::Log] {
        assert!(action.is_logged(false), "{}", action.name());
    }
    assert!(!Action::Allow.is_logged(true));
    for action in [Action::Trap(1), Action::Errno(1), Action::UserNotif, Action::Trace(1)] {
        assert!(!action.is_logged(false) && action.is_logged(true), "{}", action.name());
    }
}

#[test]
fn an_action_linux_does_not_know_kills_the_process() {
    assert_eq!(Action::of(RET_ERRNO | EACCES), Action::Errno(13));
    assert_eq!(Action::of(RET_ERRNO | 0xffff), Action::Errno(4095));
    assert_eq!(Action::of(RET_TRAP | 7), Action::Trap(7));
    assert_eq!(Action::of(RET_KILL_THREAD | 3), Action::KillThread);
    assert_eq!(Action::of(0x0001_0000), Action::KillProcess);
    assert_eq!(Action::of(0x7ffe_0000), Action::KillProcess);
    assert!(Action::is_known(RET_LOG));
    assert!(!Action::is_known(0x0001_0000));
}

#[test]
fn the_allow_list_allows_by_number_and_by_argument() {
    let program = fixture(include_bytes!("fixtures/allow-list.bpf"));
    let none = [0; 6];
    assert_eq!(program.run(&call(READ, none)), RET_ALLOW);
    assert_eq!(program.run(&call(GETPPID, none)), RET_ALLOW);
    assert_eq!(program.run(&call(IOCTL, [1, TCGETS, 0, 0, 0, 0])), RET_ALLOW);
    assert_eq!(program.run(&call(IOCTL, [1, TCGETS + 1, 0, 0, 0, 0])), RET_ERRNO | EPERM);
    // libseccomp compares all 64 bits of an argument.
    assert_eq!(program.run(&call(IOCTL, [1, TCGETS | 1 << 32, 0, 0, 0, 0])), RET_ERRNO | EPERM);
    assert_eq!(program.run(&call(SOCKET, none)), RET_KILL_PROCESS);
    assert_eq!(program.run(&call(PERSONALITY, none)), RET_ERRNO | EPERM);
    // Another arch is killed before any number is looked at.
    let x86 = SeccompData { arch: ARCH_X86, ..call(READ, none) };
    assert_eq!(Action::of(program.run(&x86)), Action::KillThread);
}

#[test]
fn the_allow_lists_map_holds_the_calls_allowed_by_number_alone() {
    let program = fixture(include_bytes!("fixtures/allow-list.bpf"));
    let calls = program.always_allows(AUDIT_ARCH_X86_64);
    for nr in [READ, 1, GETPPID, 15, 231] {
        assert!(calls.contains(nr as u64), "{nr}");
    }
    assert!(!calls.contains(IOCTL as u64));
    assert!(!calls.contains(SOCKET as u64));
    assert_eq!(calls.count(), 5);
    assert_eq!(program.always_allows(ARCH_X86), Calls::NONE);
}

#[test]
fn podmans_default_profile_gives_its_answers() {
    let program = fixture(include_bytes!("fixtures/podman-default.bpf"));
    let none = [0; 6];
    assert_eq!(program.run(&call(GETPPID, none)), RET_ALLOW);
    assert_eq!(program.run(&call(PERSONALITY, none)), RET_ALLOW);
    assert_eq!(program.run(&call(PERSONALITY, [8, 0, 0, 0, 0, 0])), RET_ALLOW);
    assert_eq!(program.run(&call(PERSONALITY, [1, 0, 0, 0, 0, 0])), RET_ERRNO | ENOSYS);
    assert_eq!(program.run(&call(BDFLUSH, none)), RET_ERRNO | EPERM);
    // A netlink audit socket is EINVAL; any other socket is allowed.
    assert_eq!(program.run(&call(SOCKET, [16, 3, 9, 0, 0, 0])), RET_ERRNO | EINVAL);
    assert_eq!(program.run(&call(SOCKET, [2, 1, 0, 0, 0, 0])), RET_ALLOW);
    // x32 calls are x86-64's numbers with bit 30 set, under x86-64's arch.
    let x32 = call(GETPPID | 0x4000_0000, none);
    assert_eq!(program.run(&x32), RET_ALLOW);
    assert_eq!(program.run(&call(999, none)), RET_ERRNO | ENOSYS);
}

#[test]
fn podmans_default_profile_skips_the_filter_for_most_calls() {
    let program = fixture(include_bytes!("fixtures/podman-default.bpf"));
    let calls = program.always_allows(AUDIT_ARCH_X86_64);
    assert!(calls.contains(GETPPID as u64));
    assert!(calls.contains(READ as u64));
    assert!(!calls.contains(PERSONALITY as u64));
    assert!(!calls.contains(SOCKET as u64));
    assert!(!calls.contains(BDFLUSH as u64));
    assert!(calls.count() > 300, "{}", calls.count());
    // What the map holds, the program allows whatever the arguments.
    let odd = [u64::MAX, 0x8000_0000, 7, 1 << 40, 3, 0];
    for nr in 0..CALLS {
        if calls.contains(nr as u64) {
            assert_eq!(program.run(&call(nr as i32, odd)), RET_ALLOW, "{nr}");
        }
    }
}

#[test]
fn a_chains_map_holds_only_what_every_filter_allows() {
    let allow_list = fixture(include_bytes!("fixtures/allow-list.bpf"));
    let podman = fixture(include_bytes!("fixtures/podman-default.bpf"));
    let both =
        allow_list.always_allows(AUDIT_ARCH_X86_64).meet(&podman.always_allows(AUDIT_ARCH_X86_64));
    assert!(both.contains(GETPPID as u64));
    assert!(!both.contains(IOCTL as u64));
    assert!(!both.contains(SOCKET as u64));
    assert!(!Calls::ALL.contains(CALLS as u64));
    assert!(!Calls::ALL.contains(u64::MAX));
}

#[test]
fn a_programs_length_is_counted_as_linux_converts_it() {
    // Each program, and the length Linux 6.18 counted for it: measured by
    // how much of the 32,768 a chain may hold it leaves.
    let allow = i(RET | K, 0, 0, RET_ALLOW);
    let refuse = i(RET | K, 0, 0, RET_ERRNO);
    let negative = 0x8000_0000;
    let cases: [(&[Instruction], u32); 20] = [
        (&[allow], 5),
        (&[i(LD | W | ABS, 0, 0, 0), allow], 6),
        (&[i(LD | IMM, 0, 0, RET_ALLOW), i(RET | A, 0, 0, 0)], 5),
        (&[i(JMP | JEQ | K, 0, 0, 5), allow], 6),
        (&[i(JMP | JEQ | K, 1, 0, 5), refuse, allow], 8),
        (&[i(JMP | JEQ | K, 0, 1, 5), refuse, allow], 8),
        (&[i(JMP | JEQ | K, 1, 1, 5), refuse, allow, allow], 11),
        (&[i(JMP | JSET | K, 0, 1, 5), refuse, allow], 9),
        (&[i(JMP | JSET | K, 1, 0, 5), refuse, allow], 8),
        (&[i(JMP | JGT | K, 0, 1, 5), refuse, allow], 8),
        (&[i(JMP | JGE | K, 0, 1, 5), refuse, allow], 8),
        (&[i(JMP | JEQ | K, 0, 0, negative), allow], 7),
        (&[i(JMP | JEQ | K, 1, 1, negative), refuse, allow, allow], 12),
        (&[i(JMP | JEQ | X, 0, 1, 0), refuse, allow], 8),
        (&[i(JMP | JEQ | X, 1, 1, 0), refuse, allow, allow], 11),
        (&[i(LDX | IMM, 0, 0, 3), i(ALU | DIV | X, 0, 0, 0), allow], 11),
        (&[i(ALU | DIV | K, 0, 0, 3), allow], 6),
        (&[i(LD | W | LEN, 0, 0, 0), allow], 6),
        (&[i(ST, 0, 0, 0), i(LD | MEM, 0, 0, 0), allow], 7),
        (&[i(MISC | TAX, 0, 0, 0), i(ALU | NEG, 0, 0, 0), i(JMP | JA, 0, 0, 0), allow], 8),
    ];
    for (program, length) in cases {
        assert_eq!(checked(program).converted_len(), length, "{program:?}");
    }
    // The longest program, 4,095 jumps and a RET: 4,100.
    let mut longest = vec![i(JMP | JA, 0, 0, 0); MAX_INSTRUCTIONS];
    longest[MAX_INSTRUCTIONS - 1] = allow;
    assert_eq!(checked(&longest).converted_len(), 4100);
}

/// A frame, as a packet capture's filter reads it: big-endian, and a load
/// past its end returns 0 from the program.
#[derive(Clone, Copy)]
struct Frame<'a>(&'a [u8]);

impl Input for Frame<'_> {
    fn length(&self) -> u32 {
        self.0.len() as u32
    }

    fn load(&self, offset: u64, size: usize) -> Option<u32> {
        let at = usize::try_from(offset).ok()?;
        let bytes = self.0.get(at..at.checked_add(size)?)?;
        Some(bytes.iter().fold(0, |word, &byte| (word << 8) | u32::from(byte)))
    }
}

fn run_on(program: &[Instruction], frame: &[u8]) -> u32 {
    machine::check(program).unwrap();
    machine::run(program, &Frame(frame))
}

#[test]
fn the_machine_loads_a_frame_and_its_ipv4_header_length() {
    let frame = [0x45, 0x12, 0x34, 0x56, 0x78];
    // Byte, half word and word, at a constant and at X + k.
    let program = [i(LD | B | ABS, 0, 0, 1), i(RET | A, 0, 0, 0)];
    assert_eq!(run_on(&program, &frame), 0x12);
    let program = [i(LDX | IMM, 0, 0, 1), i(LD | H | IND, 0, 0, 1), i(RET | A, 0, 0, 0)];
    assert_eq!(run_on(&program, &frame), 0x3456);
    let program = [i(LD | W | ABS, 0, 0, 1), i(RET | A, 0, 0, 0)];
    assert_eq!(run_on(&program, &frame), 0x1234_5678);
    let program = [i(LDX | B | 0xa0, 0, 0, 0), i(MISC | TXA, 0, 0, 0), i(RET | A, 0, 0, 0)];
    assert_eq!(run_on(&program, &frame), 20);
    // Past the end.
    let program = [i(LD | W | ABS, 0, 0, 2), i(RET | K, 0, 0, 9)];
    assert_eq!(run_on(&program, &frame), 0);
}

/// The machine's own tests, which were radix-net's when only packet
/// captures ran it: in hex, as `tcpdump -dd` prints programs.
mod machine_tests {
    use super::{Frame, INSTRUCTION, Instruction, Invalid, MAX_INSTRUCTIONS};
    use crate::machine::check;

    const fn i(code: u16, k: u32) -> Instruction {
        Instruction::new(code, k)
    }

    const fn j(code: u16, k: u32, jt: u8, jf: u8) -> Instruction {
        Instruction::jump(code, k, jt, jf)
    }

    fn whole(frame: &[u8]) -> Frame<'_> {
        Frame(frame)
    }

    fn run(program: &[Instruction], frame: Frame<'_>) -> u32 {
        crate::machine::run(program, &frame)
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
        // A shift by an X of 33 shifts by 1, as Linux's kernel runs it.
        let program = [i(0x00, 1), i(0x01, 33), i(0x6c, 0), i(0x16, 0)];
        assert_eq!(run(&program, whole(&[])), 2);
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
