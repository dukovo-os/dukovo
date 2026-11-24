// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The check that every program of the system image says it is written
//! for Dukovo.
//!
//! The kernel's `exec` gives a program Dukovo's system calls only if its
//! main ELF carries Dukovo's note or names `/system/libraries/ld.so` as
//! its interpreter; every other x86-64 program runs as a Linux program,
//! with Linux's system calls (ADR 0610). The start files of the Radix
//! toolchain (`ports/mlibc/sysdeps/radix/x86_64/crt1.S`) and libradix write
//! the note, so a program built the usual way has it. A program that lost
//! it, through a new way of linking or a binary taken from elsewhere,
//! would start and make Dukovo's calls as Linux's, so the build stops
//! first and names it.
//!
//! [`check`] reads each file as `exec` does, with the kernel's own
//! reader (`radix-elf`), and so tells what the kernel will tell.
//! [`userspace::build_for`](crate::userspace::build_for) calls it for the
//! boot archive, which holds the system's image, and
//! [`bundles`](crate::bundles) for each bundle it packs.
//!
//! A program here is a file with an execute bit that `exec` accepts as an
//! x86-64 ELF executable. A file without the bit is run by nobody: an
//! object file, or the ELF files a Go tree keeps as test data. A shared
//! library is not a program either: it is position-independent with no
//! interpreter, and its dynamic table does not flag it as an executable
//! (`DF_1_PIE`, which GNU ld and LLD set on each program they link, static
//! or dynamic, and on no library). So the dynamic linker itself, which is
//! such a library, is not checked; a program that runs as
//! `ld.so PROGRAM` would run `ld.so` as a Linux program.

use std::fmt::Write as _;
use std::fs::File;
use std::os::unix::fs::{FileExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use radix_elf::{Abi, Kind, NOTE_MAX, Program};

use crate::elf::{read_u16, read_u64};
use crate::{Context, Error, Result, ports};

/// How many bytes of a file `exec` reads first, and the most it reads to
/// hold the program headers of an ELF file (`File::open` in
/// kernel/radix/src/user/posix/exec.rs).
const HEAD: usize = 4096;
const HEAD_MAX: usize = 65536;

/// The most bytes of a dynamic table that are read: a few dozen entries.
const DYNAMIC_MAX: u64 = 4096;

/// The entry that ends a dynamic table, and the one with the flags the
/// loader reads.
const DT_NULL: u64 = 0;
const DT_FLAGS_1: u64 = 0x6fff_fffb;

/// The flag of a position-independent executable: the linker sets it on
/// a program, static or dynamic (GNU ld and LLD both do), and on no
/// shared library.
const DF_1_PIE: u64 = 0x0800_0000;

/// What `exec` makes of a file.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// Not a program: no ELF file for x86-64 that `exec` can map, a file
    /// nobody may execute, or a shared library, which only a dynamic linker
    /// loads.
    NotAProgram,
    /// A program that gets Dukovo's system calls.
    Dukovo,
    /// A program that gets Linux's, and the interpreter it names.
    Linux { interpreter: Option<String> },
}

/// Fails if a file of `files`, each a path in the image and the file on
/// the host, is a program the kernel would run as a Linux program. `what`
/// names what the files are: "the system image".
///
/// # Errors
///
/// If a file cannot be read, or programs lack the note: the error lists
/// them.
pub fn check(what: &str, files: &[(String, PathBuf)]) -> Result<()> {
    let mut linux = Vec::new();
    for (path, file) in files {
        if let Verdict::Linux { interpreter } = verdict(file)? {
            let named =
                interpreter.map_or("no interpreter".to_string(), |i| format!("interpreter {i}"));
            linux.push(format!("{path} ({named})"));
        }
    }
    if linux.is_empty() {
        return Ok(());
    }
    let mut message = format!(
        "{what} holds programs that the kernel would run as Linux programs, with Linux's system \
         calls, because they lack Dukovo's ELF note and are not started by \
         /system/libraries/ld.so (ADR 0610):"
    );
    for program in &linux {
        let _ = write!(message, "\n  {program}");
    }
    Err(Error::new(message).hint(
        "build each with the Radix toolchain, whose start files carry the note \
         (radix-posix(7), BUILDING PROGRAMS)",
    ))
}

/// [`check`] for the files beneath `folder`, each named by its path from
/// there.
///
/// # Errors
///
/// As [`check`], or if the folder cannot be read.
pub fn check_folder(what: &str, folder: &Path) -> Result<()> {
    let mut found = Vec::new();
    ports::list_files(folder, &mut found)?;
    found.sort();
    let files: Vec<(String, PathBuf)> = found
        .into_iter()
        .map(|file| {
            let inside = file.strip_prefix(folder).unwrap_or(&file).to_string_lossy().into_owned();
            (inside, file)
        })
        .collect();
    check(what, &files)
}

/// What `exec` makes of the file at `path`: the kernel's rule, read from
/// the file as `exec` reads it. A symbolic link is not looked at, for its
/// target is a file of the image as well.
fn verdict(path: &Path) -> Result<Verdict> {
    let reading = || format!("reading {}", path.display());
    let meta = std::fs::symlink_metadata(path).context(reading)?;
    // Nobody runs a file without an execute bit: an object file, or the
    // ELF files a Go tree keeps as test data.
    if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
        return Ok(Verdict::NotAProgram);
    }
    let file = File::open(path).context(reading)?;
    let head = read_head(&file, meta.len()).context(reading)?;
    // A file `exec` refuses runs under neither system.
    let Ok(program) = radix_elf::read_program(&head, meta.len()) else {
        return Ok(Verdict::NotAProgram);
    };
    if is_shared_library(&file, &program).context(reading)? {
        return Ok(Verdict::NotAProgram);
    }
    let interpreter = match program.interpreter {
        Some((offset, size)) => {
            let bytes =
                read_at(&file, offset, size, radix_elf::INTERPRETER_MAX).context(reading)?;
            // The path ends at its first NUL, as `exec` reads it.
            Some(bytes.split(|&b| b == 0).next().unwrap_or_default().to_vec())
        }
        None => None,
    };
    let mut marked = false;
    for note in program.notes() {
        let bytes = read_at(&file, note.offset, note.size, NOTE_MAX).context(reading)?;
        marked |= radix_elf::marks_dukovo(&bytes, note.align);
    }
    Ok(match Abi::of(interpreter.as_deref(), marked) {
        Abi::Dukovo => Verdict::Dukovo,
        Abi::Linux => Verdict::Linux {
            interpreter: interpreter.map(|path| String::from_utf8_lossy(&path).into_owned()),
        },
    })
}

/// The first bytes of the file, which is `size` bytes long: as many as
/// hold its ELF header and program headers, up to [`HEAD_MAX`].
fn read_head(file: &File, size: u64) -> std::io::Result<Vec<u8>> {
    let mut head = vec![0; usize::try_from(size).map_or(HEAD, |size| size.min(HEAD))];
    file.read_exact_at(&mut head, 0)?;
    if head.starts_with(b"\x7fELF")
        && let (Some(phoff), Some(phnum)) = (read_u64(&head, 32), read_u16(&head, 56))
    {
        let end = phoff.saturating_add(u64::from(phnum) * Program::HEADER_SIZE).min(size);
        if let Ok(end) = usize::try_from(end)
            && end > head.len()
            && end <= HEAD_MAX
        {
            head = vec![0; end];
            file.read_exact_at(&mut head, 0)?;
        }
    }
    Ok(head)
}

/// Up to `cap` of the `len` bytes at `offset` in the file, which the
/// reader of the headers has found to be in it.
fn read_at(file: &File, offset: u64, len: u64, cap: u64) -> std::io::Result<Vec<u8>> {
    let len = usize::try_from(len.min(cap)).map_err(std::io::Error::other)?;
    let mut bytes = vec![0; len];
    file.read_exact_at(&mut bytes, offset)?;
    Ok(bytes)
}

/// Whether `program` is a shared library: position-independent, with a
/// dynamic table, no interpreter, and no flag from its linker that it is
/// an executable ([`DF_1_PIE`]). A position-independent file without a
/// dynamic table is not taken for one, since a library has one.
fn is_shared_library(file: &File, program: &Program) -> std::io::Result<bool> {
    let (Kind::Relocatable, None, Some((offset, size))) =
        (program.image.kind, program.interpreter, program.dynamic)
    else {
        return Ok(false);
    };
    let table = read_at(file, offset, size, DYNAMIC_MAX)?;
    let entries =
        table.as_chunks::<16>().0.iter().map_while(|e| Some((read_u64(e, 0)?, read_u64(e, 8)?)));
    let executable = entries
        .take_while(|&(tag, _)| tag != DT_NULL)
        .any(|(tag, value)| tag == DT_FLAGS_1 && value & DF_1_PIE != 0);
    Ok(!executable)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use radix_elf::{DUKOVO_ABI_VERSION, DUKOVO_LOADER, DUKOVO_NOTE_NAME, NT_DUKOVO_ABI};

    use super::*;

    const ET_EXEC: u16 = 2;
    const ET_DYN: u16 = 3;
    const ET_REL: u16 = 1;

    /// Program header types.
    const PT_LOAD: u32 = 1;
    const PT_DYNAMIC: u32 = 2;
    const PT_INTERP: u32 = 3;
    const PT_NOTE: u32 = 4;

    /// Where `elf` puts a note, the interpreter's path and the dynamic
    /// table, and how long the file is.
    const NOTE_AT: usize = 0x200;
    const INTERP_AT: usize = 0x300;
    const DYNAMIC_AT: usize = 0x400;
    const FILE_LEN: usize = 0x500;

    /// A note as a linker writes it: the name and the description each
    /// padded to 4 bytes.
    fn note(name: &[u8], kind: u32, description: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u32::try_from(name.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&kind.to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        bytes.extend_from_slice(&description.to_le_bytes());
        bytes
    }

    fn dukovo_note() -> Vec<u8> {
        note(DUKOVO_NOTE_NAME, NT_DUKOVO_ABI, DUKOVO_ABI_VERSION)
    }

    /// The note GNU ld and LLD write for a build ID, which comes before
    /// Dukovo's in a segment that merges them.
    fn build_id_note() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(b"GNU\0");
        bytes.extend_from_slice(&[0xab; 8]);
        bytes
    }

    /// What a test's file has besides its one loadable segment.
    #[derive(Default)]
    struct Parts<'a> {
        /// The bytes of its note segment.
        note: Option<Vec<u8>>,
        /// Its interpreter's path, without a NUL.
        interpreter: Option<&'a [u8]>,
        /// Its dynamic table's entries.
        dynamic: Option<&'a [(u64, u64)]>,
    }

    /// An x86-64 ELF file of type `kind` that `exec` can map: one segment
    /// that holds the whole file and its entry, and the headers `parts`
    /// asks for.
    fn elf(kind: u16, parts: &Parts<'_>) -> Vec<u8> {
        let mut headers: Vec<(u32, u64, u64)> = vec![(PT_LOAD, 0, FILE_LEN as u64)];
        let mut bytes = vec![0u8; FILE_LEN];
        if let Some(note) = &parts.note {
            bytes[NOTE_AT..NOTE_AT + note.len()].copy_from_slice(note);
            headers.push((PT_NOTE, NOTE_AT as u64, note.len() as u64));
        }
        if let Some(path) = parts.interpreter {
            bytes[INTERP_AT..INTERP_AT + path.len()].copy_from_slice(path);
            headers.push((PT_INTERP, INTERP_AT as u64, path.len() as u64 + 1));
        }
        if let Some(entries) = parts.dynamic {
            for (i, (tag, value)) in entries.iter().chain(&[(DT_NULL, 0)]).enumerate() {
                let at = DYNAMIC_AT + i * 16;
                bytes[at..at + 8].copy_from_slice(&tag.to_le_bytes());
                bytes[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
            }
            headers.push((PT_DYNAMIC, DYNAMIC_AT as u64, (entries.len() as u64 + 1) * 16));
        }
        bytes[..4].copy_from_slice(b"\x7fELF");
        // 64-bit, little-endian, version 1; x86-64.
        bytes[4..7].copy_from_slice(&[2, 1, 1]);
        bytes[16..18].copy_from_slice(&kind.to_le_bytes());
        bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        let base: u64 = if kind == ET_EXEC { 0x40_0000 } else { 0 };
        bytes[24..32].copy_from_slice(&(base + 0x100).to_le_bytes());
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
        bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&u16::try_from(headers.len()).unwrap().to_le_bytes());
        for (i, &(header, offset, size)) in headers.iter().enumerate() {
            let at = 64 + i * 56;
            let (flags, vaddr) = if header == PT_LOAD { (5u32, base) } else { (4, base + offset) };
            bytes[at..at + 4].copy_from_slice(&header.to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&flags.to_le_bytes());
            bytes[at + 8..at + 16].copy_from_slice(&offset.to_le_bytes());
            bytes[at + 16..at + 24].copy_from_slice(&vaddr.to_le_bytes());
            bytes[at + 24..at + 32].copy_from_slice(&vaddr.to_le_bytes());
            bytes[at + 32..at + 40].copy_from_slice(&size.to_le_bytes());
            bytes[at + 40..at + 48].copy_from_slice(&size.to_le_bytes());
            bytes[at + 48..at + 56].copy_from_slice(&4096u64.to_le_bytes());
        }
        bytes
    }

    /// A folder under the temporary folder, empty, that no other test uses.
    fn scratch(name: &str) -> PathBuf {
        static MADE: AtomicUsize = AtomicUsize::new(0);
        let number = MADE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("ob-personality-{name}-{}-{number}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes `bytes` to `file`, which has `mode`.
    fn put(file: &Path, bytes: &[u8], mode: u32) {
        std::fs::write(file, bytes).unwrap();
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// The verdict on an executable file with these bytes.
    fn judged(bytes: &[u8]) -> Verdict {
        judged_with_mode(bytes, 0o755)
    }

    fn judged_with_mode(bytes: &[u8], mode: u32) -> Verdict {
        let dir = scratch("judged");
        let file = dir.join("program");
        put(&file, bytes, mode);
        let verdict = verdict(&file).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        verdict
    }

    fn linux(interpreter: Option<&str>) -> Verdict {
        Verdict::Linux { interpreter: interpreter.map(str::to_string) }
    }

    /// The bytes of the note section the assembly `source` writes, for the
    /// directives the start files use.
    fn assembled_note(source: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut inside = false;
        for line in source.lines().map(str::trim) {
            if line.starts_with(".section") {
                inside = line.contains(".note.dukovo.abi");
                continue;
            }
            if !inside {
                continue;
            }
            if let Some(align) = line.strip_prefix(".balign ") {
                bytes.resize(bytes.len().next_multiple_of(align.parse().unwrap()), 0);
            } else if let Some(value) = line.strip_prefix(".long ") {
                bytes.extend_from_slice(&value.parse::<u32>().unwrap().to_le_bytes());
            } else if let Some(text) = line.strip_prefix(".asciz ") {
                bytes.extend_from_slice(text.trim_matches('"').as_bytes());
                bytes.push(0);
            }
        }
        bytes
    }

    #[test]
    fn a_static_program_with_dukovo_s_note_is_dukovo_s() {
        let parts = Parts { note: Some(dukovo_note()), ..Parts::default() };
        assert_eq!(judged(&elf(ET_EXEC, &parts)), Verdict::Dukovo);
    }

    #[test]
    fn a_static_program_without_the_note_runs_as_linux() {
        assert_eq!(judged(&elf(ET_EXEC, &Parts::default())), linux(None));
        let other = Parts { note: Some(build_id_note()), ..Parts::default() };
        assert_eq!(judged(&elf(ET_EXEC, &other)), linux(None));
    }

    #[test]
    fn the_note_counts_after_a_build_id() {
        let mut both = build_id_note();
        both.extend_from_slice(&dukovo_note());
        let parts = Parts { note: Some(both), ..Parts::default() };
        assert_eq!(judged(&elf(ET_EXEC, &parts)), Verdict::Dukovo);
    }

    #[test]
    fn a_note_of_another_name_or_version_does_not_count() {
        for wrong in [
            note(b"Dukovo", NT_DUKOVO_ABI, DUKOVO_ABI_VERSION),
            note(b"Radix\0", NT_DUKOVO_ABI, DUKOVO_ABI_VERSION),
            note(DUKOVO_NOTE_NAME, NT_DUKOVO_ABI + 1, DUKOVO_ABI_VERSION),
            note(DUKOVO_NOTE_NAME, NT_DUKOVO_ABI, DUKOVO_ABI_VERSION + 1),
        ] {
            let parts = Parts { note: Some(wrong), ..Parts::default() };
            assert_eq!(judged(&elf(ET_EXEC, &parts)), linux(None));
        }
    }

    #[test]
    fn a_program_the_dynamic_linker_starts_needs_no_note() {
        let parts = Parts { interpreter: Some(DUKOVO_LOADER), ..Parts::default() };
        assert_eq!(judged(&elf(ET_DYN, &parts)), Verdict::Dukovo);
    }

    #[test]
    fn a_program_that_names_another_interpreter_runs_as_linux() {
        for path in ["/lib64/ld-linux-x86-64.so.2", "/system/libraries/ld.so.1", "/ld.so"] {
            let parts = Parts { interpreter: Some(path.as_bytes()), ..Parts::default() };
            assert_eq!(judged(&elf(ET_DYN, &parts)), linux(Some(path)));
        }
    }

    #[test]
    fn the_note_outweighs_another_interpreter() {
        let parts = Parts {
            note: Some(dukovo_note()),
            interpreter: Some(b"/lib64/ld-linux-x86-64.so.2"),
            ..Parts::default()
        };
        assert_eq!(judged(&elf(ET_DYN, &parts)), Verdict::Dukovo);
    }

    #[test]
    fn a_shared_library_is_no_program_and_a_static_pie_is() {
        let library = [(14, 1)];
        let parts = Parts { dynamic: Some(&library), ..Parts::default() };
        assert_eq!(judged(&elf(ET_DYN, &parts)), Verdict::NotAProgram);
        // A flag that is not the executable's.
        let flags = [(DT_FLAGS_1, 1)];
        let parts = Parts { dynamic: Some(&flags), ..Parts::default() };
        assert_eq!(judged(&elf(ET_DYN, &parts)), Verdict::NotAProgram);
        // The linker's mark of an executable.
        let pie = [(DT_FLAGS_1, DF_1_PIE)];
        let parts = Parts { dynamic: Some(&pie), ..Parts::default() };
        assert_eq!(judged(&elf(ET_DYN, &parts)), linux(None));
        let marked = Parts { note: Some(dukovo_note()), dynamic: Some(&pie), ..Parts::default() };
        assert_eq!(judged(&elf(ET_DYN, &marked)), Verdict::Dukovo);
        // The mark after other entries, and not past the table's end.
        let late = [(14, 1), (15, 2), (DT_FLAGS_1, DF_1_PIE)];
        let parts = Parts { dynamic: Some(&late), ..Parts::default() };
        assert_eq!(judged(&elf(ET_DYN, &parts)), linux(None));
    }

    /// glibc's libc.so.6 can be run, and names the loader as its
    /// interpreter, so a copy with an execute bit is a Linux program to
    /// this check: the Linux base bundle keeps it without the bit
    /// (ports/linux-base/build.sh), which a loader's mapping does not need.
    /// The loader itself names none and is a library.
    #[test]
    fn a_library_that_names_an_interpreter_is_a_program_only_with_an_execute_bit() {
        let flags = [(DT_FLAGS_1, 1)];
        let libc = Parts {
            interpreter: Some(b"/lib64/ld-linux-x86-64.so.2"),
            dynamic: Some(&flags),
            ..Parts::default()
        };
        let libc = elf(ET_DYN, &libc);
        assert_eq!(judged(&libc), linux(Some("/lib64/ld-linux-x86-64.so.2")));
        assert_eq!(judged_with_mode(&libc, 0o644), Verdict::NotAProgram);
        let loader = Parts { dynamic: Some(&flags), ..Parts::default() };
        assert_eq!(judged(&elf(ET_DYN, &loader)), Verdict::NotAProgram);
    }

    #[test]
    fn a_position_independent_file_without_a_dynamic_table_is_a_program() {
        assert_eq!(judged(&elf(ET_DYN, &Parts::default())), linux(None));
    }

    #[test]
    fn what_exec_cannot_run_is_no_program() {
        assert_eq!(judged(b""), Verdict::NotAProgram);
        assert_eq!(judged(b"#!/bin/sh\necho hello\n"), Verdict::NotAProgram);
        assert_eq!(judged(&elf(ET_REL, &Parts::default())), Verdict::NotAProgram);
        // A program for another machine.
        let mut arm = elf(ET_EXEC, &Parts::default());
        arm[18..20].copy_from_slice(&183u16.to_le_bytes());
        assert_eq!(judged(&arm), Verdict::NotAProgram);
        // Cut off in its program headers.
        assert_eq!(judged(&elf(ET_EXEC, &Parts::default())[..100]), Verdict::NotAProgram);
    }

    #[test]
    fn a_file_nobody_may_execute_is_no_program() {
        let bare = elf(ET_EXEC, &Parts::default());
        assert_eq!(judged_with_mode(&bare, 0o644), Verdict::NotAProgram);
        assert_eq!(judged_with_mode(&bare, 0o744), linux(None));
        // Any one execute bit is enough; the owner may still read it, as
        // the check runs as an ordinary account in CI.
        assert_eq!(judged_with_mode(&bare, 0o401), linux(None));
    }

    #[test]
    fn a_note_segment_is_read_up_to_its_limit() {
        // Dukovo's note first, in a segment longer than the limit.
        let mut long = dukovo_note();
        long.resize(NOTE_MAX as usize + 64, 0);
        let parts = Parts { note: Some(long), ..Parts::default() };
        assert_eq!(judged(&elf(ET_EXEC, &parts)), Verdict::Dukovo);
        // After the limit, it is not seen.
        let mut late = vec![0; NOTE_MAX as usize];
        late.extend_from_slice(&dukovo_note());
        let parts = Parts { note: Some(late), ..Parts::default() };
        assert_eq!(judged(&elf(ET_EXEC, &parts)), linux(None));
    }

    #[test]
    fn the_start_files_write_the_note_the_kernel_looks_for() {
        let crt1 = include_str!("../../../ports/mlibc/sysdeps/radix/x86_64/crt1.S");
        let libradix = include_str!("../../../userspace/libradix/src/lib.rs");
        for (name, source) in [("crt1.S", crt1), ("libradix", libradix)] {
            let bytes = assembled_note(source);
            assert_eq!(bytes, dukovo_note(), "the note of {name}");
            assert!(radix_elf::marks_dukovo(&bytes, 4), "the kernel reads the note of {name}");
        }
    }

    #[test]
    fn the_check_lists_every_linux_program_and_no_other() {
        let dir = scratch("listing");
        let write = |name: &str, bytes: &[u8]| {
            let file = dir.join(name);
            put(&file, bytes, 0o755);
            (format!("system/commands/{name}"), file)
        };
        let marked = Parts { note: Some(dukovo_note()), ..Parts::default() };
        let loader = Parts { interpreter: Some(DUKOVO_LOADER), ..Parts::default() };
        let library = [(14, 1)];
        let shared = Parts { dynamic: Some(&library), ..Parts::default() };
        let files = [
            write("static", &elf(ET_EXEC, &marked)),
            write("dynamic", &elf(ET_DYN, &loader)),
            write("libc.so", &elf(ET_DYN, &shared)),
            write("script", b"#!/bin/sh\n"),
        ];
        assert!(check("the image", &files).is_ok());

        let foreign =
            Parts { interpreter: Some(b"/lib64/ld-linux-x86-64.so.2"), ..Parts::default() };
        let mut files = Vec::from(files);
        files.push(write("glibc", &elf(ET_DYN, &foreign)));
        files.push(write("bare", &elf(ET_EXEC, &Parts::default())));
        std::os::unix::fs::symlink("bare", dir.join("alias")).unwrap();
        files.push(("system/commands/alias".to_string(), dir.join("alias")));
        let err = check("the image", &files).unwrap_err().to_string();
        assert!(err.starts_with("the image holds programs that the kernel would run as Linux"));
        assert!(
            err.contains("\n  system/commands/glibc (interpreter /lib64/ld-linux-x86-64.so.2)")
        );
        assert!(err.contains("\n  system/commands/bare (no interpreter)"));
        // The others, and a link to one, are not listed.
        assert_eq!(err.lines().count(), 3, "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_is_checked_with_each_file_named_from_it() {
        let dir = scratch("folder");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::create_dir_all(dir.join("lib/plugins")).unwrap();
        let marked = Parts { note: Some(dukovo_note()), ..Parts::default() };
        put(&dir.join("bin/tool"), &elf(ET_EXEC, &marked), 0o755);
        put(&dir.join("README"), b"text", 0o644);
        // A file of test data that is an ELF program but is not executable.
        let bare = elf(ET_EXEC, &Parts::default());
        put(&dir.join("lib/plugins/testdata"), &bare, 0o644);
        assert!(check_folder("the bundle", &dir).is_ok());

        put(&dir.join("lib/plugins/helper"), &bare, 0o755);
        let err = check_folder("the bundle of port tool", &dir).unwrap_err().to_string();
        assert!(err.contains("\n  lib/plugins/helper (no interpreter)"), "{err}");
        assert!(!err.contains("bin/tool") && !err.contains("testdata"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
