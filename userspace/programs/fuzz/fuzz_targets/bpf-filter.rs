// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The classic BPF programs a Linux program hands `seccomp(2)` and a
//! packet capture's reader hands its capture file, which radix-bpf checks
//! and runs in the kernel (ADRs 0346, 0623). The input is a call's
//! `struct seccomp_data` (its first 64 bytes) and then a program, 8 bytes
//! an instruction. No program makes a check, a run or the cache of the
//! calls a filter always allows panic; a filter the cache would skip for
//! a call allows that call; a program seccomp takes is one the machine
//! takes; and its length as Linux converts it is within its bounds.

#![forbid(unsafe_code)]
#![no_main]

use libfuzzer_sys::fuzz_target;
use radix_bpf::seccomp::{RET_ALLOW, SECCOMP_DATA_SIZE};
use radix_bpf::{INSTRUCTION, Input, Instruction, MAX_INSTRUCTIONS, Program, SeccompData};

/// The program's bytes, as a frame for the machine's loads: big-endian,
/// as a packet capture's frames are read.
struct Frame<'a>(&'a [u8]);

impl Input for Frame<'_> {
    fn len(&self) -> u32 {
        u32::try_from(self.0.len()).unwrap_or(u32::MAX)
    }

    fn load(&self, offset: u64, size: usize) -> Option<u32> {
        let at = usize::try_from(offset).ok()?;
        let bytes = self.0.get(at..at.checked_add(size)?)?;
        Some(bytes.iter().fold(0, |word, &byte| (word << 8) | u32::from(byte)))
    }
}

/// The call the first 64 bytes describe.
fn call(bytes: &[u8; SECCOMP_DATA_SIZE]) -> SeccompData {
    let word = |at: usize| u64::from_ne_bytes(bytes[at..at + 8].try_into().expect("8 bytes"));
    SeccompData {
        nr: i32::from_ne_bytes(bytes[0..4].try_into().expect("4 bytes")),
        arch: u32::from_ne_bytes(bytes[4..8].try_into().expect("4 bytes")),
        instruction_pointer: word(8),
        args: core::array::from_fn(|i| word(16 + 8 * i)),
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((head, rest)) = data.split_first_chunk::<SECCOMP_DATA_SIZE>() else { return };
    let data = call(head);
    let instructions: Vec<Instruction> = rest
        .as_chunks::<INSTRUCTION>()
        .0
        .iter()
        .take(MAX_INSTRUCTIONS + 1)
        .map(|bytes| Instruction::from_bytes(*bytes))
        .collect();

    // Any program on a frame, if the machine takes it.
    if radix_bpf::check(&instructions).is_ok() {
        let _ = radix_bpf::run(&instructions, &Frame(rest));
    }

    let mut masks = vec![0u16; instructions.len()];
    let Ok(program) = Program::check(&instructions[..], &mut masks) else { return };
    assert!(radix_bpf::check(program.instructions()).is_ok());
    // Linux's conversion makes three instructions first, and at most five
    // of each.
    let converted = program.converted_len() as usize;
    assert!(converted >= program.len() + 3 && converted <= 5 * program.len() + 3);
    let value = program.run(&data);
    let allowed = program.always_allows(data.arch);
    if allowed.contains(u64::from(data.nr.cast_unsigned())) {
        assert_eq!(value, RET_ALLOW);
    }
});
