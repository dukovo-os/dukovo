// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The ELF header of the program `linux run` runs: whether it is an
//! x86-64 executable, and whether it names an interpreter.
//!
//! The command reads the first bytes of the file before it builds a
//! sandbox, so that a file no x86-64 Linux would run is refused in one
//! line, and a static program, which needs no Linux base, runs without
//! one. The kernel checks everything again at `exec` (`radix-elf`); this
//! reads only what the choice of sandbox depends on.

/// The most bytes of the file that [`inspect`] looks at: as many as the
/// kernel reads for the program headers of any program it runs.
pub const HEAD_MAX: usize = 65536;

const MAGIC: &[u8] = b"\x7fELF";
/// ELF's `EI_CLASS` for 64-bit objects, `EI_DATA` for little-endian ones.
const CLASS_64: u8 = 2;
const LITTLE_ENDIAN: u8 = 1;
const HEADER_SIZE: usize = 64;
const PHDR_SIZE: usize = 56;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;
const PT_INTERP: u32 = 3;

/// How a program is linked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Linking {
    /// With no interpreter: it runs on its own, as a position-independent
    /// static program does too.
    Static,
    /// With an interpreter (`PT_INTERP`), a dynamic loader that maps its
    /// libraries from the Linux base.
    Dynamic,
}

/// Whether the file that starts with `head` is an x86-64 executable
/// (`ET_EXEC`, or `ET_DYN` as position-independent programs are), and
/// how it is linked.
///
/// # Errors
///
/// A line that says why the file does not run: it is no ELF file, or it is
/// not 64-bit, little-endian, built for x86-64, or an executable.
pub fn inspect(head: &[u8]) -> Result<Linking, String> {
    if !head.starts_with(MAGIC) || head.len() < HEADER_SIZE {
        return Err("it is not an ELF program".into());
    }
    if head[4] != CLASS_64 {
        return Err("it is a 32-bit program, and only x86-64 programs run".into());
    }
    if head[5] != LITTLE_ENDIAN {
        return Err("it is not little-endian, and only x86-64 programs run".into());
    }
    let machine = u16_at(head, 18);
    if machine != EM_X86_64 {
        return Err(format!(
            "it is built for another processor (machine {machine}), and only x86-64 programs run"
        ));
    }
    let kind = u16_at(head, 16);
    if kind != ET_EXEC && kind != ET_DYN {
        return Err(format!("it is not an executable (ELF type {kind})"));
    }
    let count = usize::from(u16_at(head, 56));
    if count > 0 && usize::from(u16_at(head, 54)) != PHDR_SIZE {
        return Err("its program headers are not the size x86-64 programs have".into());
    }
    let table = usize::try_from(u64_at(head, 32)).ok().and_then(|at| {
        let end = at.checked_add(count * PHDR_SIZE)?;
        head.get(at..end)
    });
    let Some(headers) = table else {
        return Err("its program headers are cut off".into());
    };
    let names_interpreter =
        headers.as_chunks::<PHDR_SIZE>().0.iter().any(|header| u32_at(header, 0) == PT_INTERP);
    Ok(if names_interpreter { Linking::Dynamic } else { Linking::Static })
}

// The callers have checked that the bytes reach the field.
fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(core::array::from_fn(|i| bytes[at + i]))
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(core::array::from_fn(|i| bytes[at + i]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header with one program header for each type in `types`.
    fn file(class: u8, data: u8, kind: u16, machine: u16, types: &[u32]) -> Vec<u8> {
        let mut bytes = vec![0u8; HEADER_SIZE + types.len() * PHDR_SIZE];
        bytes[..4].copy_from_slice(MAGIC);
        bytes[4] = class;
        bytes[5] = data;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&kind.to_le_bytes());
        bytes[18..20].copy_from_slice(&machine.to_le_bytes());
        bytes[32..40].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes());
        bytes[54..56].copy_from_slice(&(PHDR_SIZE as u16).to_le_bytes());
        bytes[56..58].copy_from_slice(&(types.len() as u16).to_le_bytes());
        for (i, kind) in types.iter().enumerate() {
            let at = HEADER_SIZE + i * PHDR_SIZE;
            bytes[at..at + 4].copy_from_slice(&kind.to_le_bytes());
        }
        bytes
    }

    const PT_LOAD: u32 = 1;
    const PT_DYNAMIC: u32 = 2;
    const PT_PHDR: u32 = 6;

    #[test]
    fn a_program_with_no_interpreter_is_static() {
        let exec = file(CLASS_64, LITTLE_ENDIAN, ET_EXEC, EM_X86_64, &[PT_LOAD]);
        assert_eq!(inspect(&exec), Ok(Linking::Static));
        // A static position-independent program has a dynamic section and
        // no interpreter.
        let pie = file(CLASS_64, LITTLE_ENDIAN, ET_DYN, EM_X86_64, &[PT_LOAD, PT_DYNAMIC]);
        assert_eq!(inspect(&pie), Ok(Linking::Static));
    }

    #[test]
    fn a_program_that_names_an_interpreter_is_dynamic() {
        let types = [PT_PHDR, PT_INTERP, PT_LOAD, PT_DYNAMIC];
        let dynamic = file(CLASS_64, LITTLE_ENDIAN, ET_DYN, EM_X86_64, &types);
        assert_eq!(inspect(&dynamic), Ok(Linking::Dynamic));
        let fixed = file(CLASS_64, LITTLE_ENDIAN, ET_EXEC, EM_X86_64, &[PT_INTERP, PT_LOAD]);
        assert_eq!(inspect(&fixed), Ok(Linking::Dynamic));
    }

    #[test]
    fn what_is_no_x86_64_executable_is_refused_with_a_reason() {
        let refused = |bytes: Vec<u8>| inspect(&bytes).unwrap_err();
        assert_eq!(refused(b"#!/bin/sh\necho hello\n".to_vec()), "it is not an ELF program");
        assert_eq!(refused(Vec::new()), "it is not an ELF program");
        let short = file(CLASS_64, LITTLE_ENDIAN, ET_EXEC, EM_X86_64, &[])[..40].to_vec();
        assert_eq!(refused(short), "it is not an ELF program");
        assert_eq!(
            refused(file(1, LITTLE_ENDIAN, ET_EXEC, 3, &[PT_LOAD])),
            "it is a 32-bit program, and only x86-64 programs run"
        );
        assert_eq!(
            refused(file(CLASS_64, 2, ET_EXEC, EM_X86_64, &[PT_LOAD])),
            "it is not little-endian, and only x86-64 programs run"
        );
        assert_eq!(
            refused(file(CLASS_64, LITTLE_ENDIAN, ET_EXEC, 183, &[PT_LOAD])),
            "it is built for another processor (machine 183), and only x86-64 programs run"
        );
        // A relocatable object, and a core file.
        for kind in [1, 4] {
            assert_eq!(
                refused(file(CLASS_64, LITTLE_ENDIAN, kind, EM_X86_64, &[PT_LOAD])),
                format!("it is not an executable (ELF type {kind})")
            );
        }
    }

    #[test]
    fn program_headers_that_run_past_the_bytes_read_are_cut_off() {
        let mut bytes = file(CLASS_64, LITTLE_ENDIAN, ET_EXEC, EM_X86_64, &[PT_LOAD, PT_INTERP]);
        bytes.truncate(HEADER_SIZE + PHDR_SIZE + 10);
        assert_eq!(inspect(&bytes), Err("its program headers are cut off".into()));
        // A table that starts past the end, or whose size wraps.
        let mut far = file(CLASS_64, LITTLE_ENDIAN, ET_EXEC, EM_X86_64, &[PT_LOAD]);
        far[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(inspect(&far).is_err());
    }
}
