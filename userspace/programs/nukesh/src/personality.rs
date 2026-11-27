// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Which programs the shell runs through `linux run` (ADR 0622): the
//! dynamic programs written for Linux, whose libraries and `/proc` the
//! Linux view gives them, and which the kernel cannot start on their own,
//! as their dynamic linker is not in Dukovo's root.
//!
//! The rule is the kernel's (`radix-elf`, whose reader and note check this
//! uses): a program is Linux's unless it names Dukovo's dynamic linker or
//! carries Dukovo's note, and only one that names an interpreter has
//! libraries to load. A static Linux program runs as it is, the kernel's
//! Linux table serving it. A file whose headers or notes the first bytes do
//! not show is run as it always was, not through the view: it is the one
//! the shell cannot tell is Linux's. In a Linux view already, the shell
//! starts every program as it is (ADR 0626).

use std::ffi::{CStr, CString};

use radix_elf::{Abi, NOTE_MAX};

/// How much of the file the shell reads to tell: as the kernel does, the
/// first bytes hold the headers and, where linkers put them, the notes.
pub const HEAD: usize = 4096;

/// The command that runs a program in a Linux view.
pub const LINUX: &CStr = c"/system/commands/linux";

/// The variable a Linux view's programs find set (ADR 0626).
pub const IN_VIEW: &str = "DUKOVO_LINUX_VIEW";

/// Whether the shell runs in a Linux view, as the view's `/bin/sh`: a
/// Linux program it starts runs there as it is, the kernel finding its
/// dynamic linker in the view and linuxd serving it.
pub fn in_linux_view() -> bool {
    std::env::var_os(IN_VIEW).is_some()
}

/// Whether the file of `size` bytes that starts with `head` is a program
/// for Linux that names an interpreter, and so runs in a Linux view.
pub fn runs_in_linux_view(head: &[u8], size: u64) -> bool {
    let Ok(program) = radix_elf::read_program(head, size) else { return false };
    let Some((offset, len)) = program.interpreter else { return false };
    let Some(path) = bytes_at(head, offset, len) else { return false };
    // The path ends at its NUL, as the kernel reads it.
    let path = path.split(|&b| b == 0).next().unwrap_or(path);
    let mut marked = false;
    for note in program.notes() {
        let Some(bytes) = bytes_at(head, note.offset, note.size.min(NOTE_MAX)) else {
            return false;
        };
        marked |= radix_elf::marks_dukovo(bytes, note.align);
    }
    Abi::of(Some(path), marked) == Abi::Linux
}

/// The `len` bytes at `offset` in `head`, if it holds them.
fn bytes_at(head: &[u8], offset: u64, len: u64) -> Option<&[u8]> {
    let start = usize::try_from(offset).ok()?;
    head.get(start..start.checked_add(usize::try_from(len).ok()?)?)
}

/// What the shell runs for the program at `path` that `argv` names: `linux
/// run -- PATH ARGUMENT...`. The program's own name, `argv[0]`, is the
/// path: `linux run` starts it as it is given.
pub fn linux_command(path: &CStr, argv: &[CString]) -> Vec<CString> {
    let mut command: Vec<CString> =
        ["linux", "run", "--"].iter().filter_map(|word| CString::new(*word).ok()).collect();
    command.push(path.to_owned());
    command.extend(argv.iter().skip(1).cloned());
    command
}

#[cfg(test)]
mod tests {
    use radix_elf::{DUKOVO_ABI_VERSION, DUKOVO_LOADER, DUKOVO_NOTE_NAME, NT_DUKOVO_ABI};

    use super::*;

    const HEADER_SIZE: usize = 64;
    const PHDR_SIZE: usize = 56;
    const ET_DYN: u16 = 3;
    const EM_X86_64: u16 = 62;
    const PT_INTERP: u32 = 3;
    const PT_NOTE: u32 = 4;

    /// Whether the head is a program that runs in the view, as a file of
    /// its own length, and of a megabyte, which a program's segments may
    /// reach into beyond the head.
    fn runs(head: &[u8]) -> bool {
        let short = runs_in_linux_view(head, head.len() as u64);
        assert_eq!(short, runs_in_linux_view(head, 1 << 20), "the file's size matters");
        short
    }

    /// A note as the ELF format lays it out, its name and description
    /// padded to `align`.
    fn note(name: &[u8], kind: u32, desc: &[u8], align: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(desc.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&kind.to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.resize(bytes.len().next_multiple_of(align), 0);
        bytes.extend_from_slice(desc);
        bytes.resize(bytes.len().next_multiple_of(align), 0);
        bytes
    }

    fn dukovo_note(align: usize) -> Vec<u8> {
        note(DUKOVO_NOTE_NAME, NT_DUKOVO_ABI, &DUKOVO_ABI_VERSION.to_le_bytes(), align)
    }

    /// The head of an x86-64 program with an interpreter, if it has one,
    /// and a note segment for each of `notes`, aligned to 4.
    fn program(interpreter: Option<&[u8]>, notes: &[Vec<u8>]) -> Vec<u8> {
        let mut segments: Vec<(u32, Vec<u8>)> = Vec::new();
        if let Some(path) = interpreter {
            let mut text = path.to_vec();
            text.push(0);
            segments.push((PT_INTERP, text));
        }
        segments.extend(notes.iter().map(|bytes| (PT_NOTE, bytes.clone())));
        let mut head = vec![0u8; HEADER_SIZE + (segments.len() + 1) * PHDR_SIZE];
        head[..4].copy_from_slice(b"\x7fELF");
        head[4..7].copy_from_slice(&[2, 1, 1]);
        head[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
        head[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
        head[32..40].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes());
        head[54..56].copy_from_slice(&(PHDR_SIZE as u16).to_le_bytes());
        head[56..58].copy_from_slice(&((segments.len() + 1) as u16).to_le_bytes());
        // A loadable segment of code, as every program has, which holds the
        // entry point, 0: read and execute, a page from the file's start.
        head[HEADER_SIZE..HEADER_SIZE + 4].copy_from_slice(&1u32.to_le_bytes());
        head[HEADER_SIZE + 4..HEADER_SIZE + 8].copy_from_slice(&5u32.to_le_bytes());
        head[HEADER_SIZE + 32..HEADER_SIZE + 40].copy_from_slice(&64u64.to_le_bytes());
        head[HEADER_SIZE + 40..HEADER_SIZE + 48].copy_from_slice(&4096u64.to_le_bytes());
        for (i, (kind, bytes)) in segments.iter().enumerate() {
            let at = HEADER_SIZE + (i + 1) * PHDR_SIZE;
            let offset = head.len() as u64;
            head[at..at + 4].copy_from_slice(&kind.to_le_bytes());
            head[at + 8..at + 16].copy_from_slice(&offset.to_le_bytes());
            head[at + 32..at + 40].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
            head[at + 48..at + 56].copy_from_slice(&4u64.to_le_bytes());
            head.extend_from_slice(bytes);
        }
        head
    }

    const GLIBC: &[u8] = b"/lib64/ld-linux-x86-64.so.2";
    const MUSL: &[u8] = b"/lib/ld-musl-x86_64.so.1";

    #[test]
    fn a_dukovo_program_runs_as_it_always_did() {
        // Its dynamic linker, with or without the note, and the note
        // alone, as a static one has it.
        assert!(!runs(&program(Some(DUKOVO_LOADER), &[])));
        assert!(!runs(&program(Some(DUKOVO_LOADER), &[dukovo_note(4)])));
        assert!(!runs(&program(None, &[dukovo_note(4)])));
        // The note makes a program Dukovo's, whatever its interpreter.
        assert!(!runs(&program(Some(GLIBC), &[dukovo_note(4)])));
    }

    #[test]
    fn a_static_linux_program_runs_as_it_always_did() {
        assert!(!runs(&program(None, &[])));
        let build_id = note(b"GNU\0", 3, &[0xab; 20], 4);
        assert!(!runs(&program(None, &[build_id])));
    }

    #[test]
    fn a_dynamic_glibc_program_runs_in_the_view() {
        assert!(runs(&program(Some(GLIBC), &[])));
        let build_id = note(b"GNU\0", 3, &[0xab; 20], 4);
        assert!(runs(&program(Some(GLIBC), std::slice::from_ref(&build_id))));
        // A build ID before Dukovo's note does not hide it.
        let mut both = build_id;
        both.extend_from_slice(&dukovo_note(4));
        assert!(!runs(&program(Some(GLIBC), &[both])));
    }

    #[test]
    fn a_dynamic_musl_program_runs_in_the_view() {
        assert!(runs(&program(Some(MUSL), &[])));
    }

    #[test]
    fn what_is_no_x86_64_program_is_left_alone() {
        assert!(!runs(b""));
        assert!(!runs(b"#!/bin/sh\necho hello\n"));
        assert!(!runs(b"\x7fELF"));
        let linux = program(Some(GLIBC), &[]);
        for (at, value) in
            [(4usize, 1u8), (5, 2), (6, 2), (16, 1), (16, 4), (18, 183), (18, 3), (54, 32)]
        {
            let mut other = linux.clone();
            other[at] = value;
            assert!(!runs(&other), "byte {at} is {value}");
        }
    }

    #[test]
    fn what_the_first_bytes_do_not_show_is_left_alone() {
        let linux = program(Some(GLIBC), &[]);
        // Cut off in its headers, or where the interpreter is.
        assert!(!runs(&linux[..HEADER_SIZE + PHDR_SIZE]));
        assert!(!runs(&linux[..linux.len() - 5]));
        // A note beyond them, which may be Dukovo's.
        let phdr = |i: usize| HEADER_SIZE + (i + 1) * PHDR_SIZE;
        let mut far = program(None, &[dukovo_note(4)]);
        far[phdr(0) + 8..phdr(0) + 16].copy_from_slice(&1_000_000u64.to_le_bytes());
        assert!(!runs(&far));
        let mut far = program(Some(GLIBC), &[dukovo_note(4)]);
        far[phdr(1) + 8..phdr(1) + 16].copy_from_slice(&1_000_000u64.to_le_bytes());
        assert!(!runs(&far));
    }

    #[test]
    fn the_command_runs_the_path_with_the_arguments_after_it() {
        let argv: Vec<CString> =
            ["hello", "-v", "a b"].iter().map(|word| CString::new(*word).unwrap()).collect();
        let command = linux_command(c"/applications/commands/hello", &argv);
        let words: Vec<&str> = command.iter().map(|word| word.to_str().unwrap()).collect();
        assert_eq!(words, ["linux", "run", "--", "/applications/commands/hello", "-v", "a b"]);
        // A program with no argv at all.
        assert_eq!(linux_command(c"/x", &[]).len(), 4);
    }
}
