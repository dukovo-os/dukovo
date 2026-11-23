// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Builds the vDSO (ADR 0616) from `src/vdso.c` with Clang and links it
//! with the Rust toolchain's own LLD into a shared object whose one
//! segment is read and execute, laid out by `src/vdso.lds`, with the names
//! `src/vdso.map` exports. The image is then checked: it must work
//! wherever it is mapped, with nothing to relocate and nothing to write,
//! and export every name. The kernel maps it as it is.
//!
//! Clang is `$RADIX_CLANG`, which `ob` sets to the Clang the ports are
//! built with, or else the newest `clang-N` on `PATH`, or `clang`, as for
//! uACPI. The linker is `$RADIX_LLD`, or the toolchain's `rust-lld`.
//!
//! Only x86-64 has a vDSO: for other targets the image is empty.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The oldest Clang, as for the ports.
const CLANG_MIN: u32 = 20;

/// The flags of the compile: position independent, freestanding, with
/// unwind tables for debuggers, and no vector registers, so that a call
/// touches only what a function call may.
const CFLAGS: &[&str] = &[
    "--target=x86_64-unknown-none-elf",
    "-std=c11",
    "-O2",
    "-fPIC",
    "-ffreestanding",
    "-fno-stack-protector",
    "-mno-red-zone",
    "-mgeneral-regs-only",
    "-fvisibility=hidden",
    "-fasynchronous-unwind-tables",
    "-Wall",
    "-Wextra",
    "-Werror",
];

/// The flags of the link: a shared object with both hash tables (Linux's
/// C libraries and Go's runtime look names up through either), no build
/// ID, which would differ from build to build of the same source, and no
/// symbol table beyond the dynamic one.
const LDFLAGS: &[&str] = &[
    "-flavor",
    "gnu",
    "-shared",
    "-soname=linux-vdso.so.1",
    "--hash-style=both",
    "--build-id=none",
    "-z",
    "max-page-size=4096",
    "-z",
    "noexecstack",
    "-Bsymbolic",
    "--no-undefined",
    "--eh-frame-hdr",
    "--strip-all",
];

/// The names the image must export.
const EXPORTS: &[&str] = &[
    "__vdso_clock_gettime",
    "clock_gettime",
    "__vdso_gettimeofday",
    "gettimeofday",
    "__vdso_time",
    "time",
    "__vdso_clock_getres",
    "clock_getres",
    "__vdso_getcpu",
    "getcpu",
    "__dukovo_clock_read",
];

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/vdso.c");
    println!("cargo:rerun-if-changed=src/clock_page.h");
    println!("cargo:rerun-if-changed=src/vdso.lds");
    println!("cargo:rerun-if-changed=src/vdso.map");
    println!("cargo:rerun-if-env-changed=RADIX_CLANG");
    println!("cargo:rerun-if-env-changed=RADIX_LLD");
    let image = out.join("vdso.so");
    if env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default() != "x86_64" {
        std::fs::write(&image, []).expect("writing the empty image");
        return;
    }
    let src = dir.join("src");
    let object = out.join("vdso.o");
    run(Command::new(clang())
        .args(CFLAGS)
        .arg("-c")
        .arg(src.join("vdso.c"))
        .arg("-o")
        .arg(&object));
    run(Command::new(lld())
        .args(LDFLAGS)
        .arg("-T")
        .arg(src.join("vdso.lds"))
        .arg(format!("--version-script={}", src.join("vdso.map").display()))
        .arg(&object)
        .arg("-o")
        .arg(&image));
    let bytes = std::fs::read(&image).expect("reading the image");
    if let Err(problem) = check(&bytes) {
        let _ = std::fs::remove_file(&image);
        panic!("the vDSO image is not one the kernel may map: {problem}");
    }
}

fn run(command: &mut Command) {
    let status = command.status().unwrap_or_else(|err| panic!("cannot run {command:?}: {err}"));
    assert!(status.success(), "{command:?} failed: {status}");
}

/// `$RADIX_CLANG`, or the newest `clang-N` on `PATH` from [`CLANG_MIN`]
/// up, or `clang`.
fn clang() -> PathBuf {
    if let Some(path) = env::var_os("RADIX_CLANG") {
        return PathBuf::from(path);
    }
    let paths: Vec<PathBuf> =
        env::var_os("PATH").map_or_default(|p| env::split_paths(&p).collect());
    let find = |program: &str| paths.iter().map(|dir| dir.join(program)).find(|p| p.is_file());
    (CLANG_MIN..CLANG_MIN + 20)
        .rev()
        .find_map(|major| find(&format!("clang-{major}")))
        .or_else(|| find("clang"))
        .unwrap_or_else(|| panic!("the vDSO needs Clang {CLANG_MIN} or later; run `./ob doctor`"))
}

/// `$RADIX_LLD`, or the `rust-lld` of the toolchain building this crate.
fn lld() -> PathBuf {
    if let Some(path) = env::var_os("RADIX_LLD") {
        return PathBuf::from(path);
    }
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let host = env::var("HOST").expect("cargo sets HOST");
    let output = Command::new(&rustc)
        .args(["--print", "sysroot"])
        .output()
        .unwrap_or_else(|err| panic!("cannot run {rustc:?}: {err}"));
    let sysroot = String::from_utf8(output.stdout).expect("the sysroot is a path");
    let lld = Path::new(sysroot.trim()).join("lib/rustlib").join(host).join("bin/rust-lld");
    assert!(lld.is_file(), "the toolchain has no rust-lld at {}", lld.display());
    lld
}

/// Whether `image` is a vDSO the kernel may map as it is: an x86-64
/// shared object with one loadable segment, read and execute, starting at
/// its first byte and address 0, holding every allocated section, nothing
/// writable, no relocation and no library it needs, and every name of
/// [`EXPORTS`] in its dynamic symbols, with no other.
fn check(image: &[u8]) -> Result<(), String> {
    let elf = Elf { bytes: image };
    if image.get(..4) != Some(b"\x7fELF".as_slice()) || image.get(4..6) != Some(&[2, 1]) {
        return Err("not a 64-bit little-endian ELF file".into());
    }
    if elf.u16(16)? != 3 || elf.u16(18)? != 62 {
        return Err("not an x86-64 shared object".into());
    }
    const PT_LOAD: u32 = 1;
    const PT_DYNAMIC: u32 = 2;
    const PF_W: u32 = 2;
    let (phoff, phnum) = (elf.u64(32)?, elf.u16(56)?);
    let mut load = None;
    let mut dynamic = None;
    for index in 0..u64::from(phnum) {
        let at = phoff + index * 56;
        let (kind, flags) = (elf.u32(at)?, elf.u32(at + 4)?);
        let (offset, vaddr, filesz, memsz) =
            (elf.u64(at + 8)?, elf.u64(at + 16)?, elf.u64(at + 32)?, elf.u64(at + 40)?);
        if flags & PF_W != 0 {
            return Err(format!("segment {index} is writable"));
        }
        match kind {
            PT_LOAD if load.is_some() => return Err("more than one loadable segment".into()),
            PT_LOAD => {
                if offset != 0 || vaddr != 0 || filesz != memsz || flags != 5 {
                    return Err(
                        "the loadable segment is not the image as it is, read and execute".into()
                    );
                }
                load = Some(filesz);
            }
            PT_DYNAMIC => dynamic = Some((offset, filesz)),
            _ => {}
        }
    }
    let size = load.ok_or("no loadable segment")?;
    if size > image.len() as u64 {
        return Err("the segment runs past the file".into());
    }
    // Every allocated section is in the segment, and only `.dynamic`, which
    // nothing writes here, says it may be written.
    const SHF_WRITE: u64 = 1;
    const SHF_ALLOC: u64 = 2;
    const SHT_NOBITS: u32 = 8;
    let (shoff, shnum, shstrndx) = (elf.u64(40)?, elf.u16(60)?, elf.u16(62)?);
    let names = elf.u64(shoff + u64::from(shstrndx) * 64 + 24)?;
    for index in 0..u64::from(shnum) {
        let at = shoff + index * 64;
        let name = elf.string(names + u64::from(elf.u32(at)?))?;
        let (kind, flags, addr, len) =
            (elf.u32(at + 4)?, elf.u64(at + 8)?, elf.u64(at + 16)?, elf.u64(at + 32)?);
        if flags & SHF_ALLOC == 0 {
            continue;
        }
        if kind == SHT_NOBITS || addr + len > size {
            return Err(format!("section {name} is not in the image"));
        }
        if flags & SHF_WRITE != 0 && name != ".dynamic" {
            return Err(format!("section {name} is writable"));
        }
    }
    // No relocation, no library, and the tables a lookup needs.
    const DT_NEEDED: u64 = 1;
    const DT_HASH: u64 = 4;
    const DT_STRTAB: u64 = 5;
    const DT_SYMTAB: u64 = 6;
    const DT_RELA: u64 = 7;
    const DT_REL: u64 = 17;
    const DT_TEXTREL: u64 = 22;
    const DT_JMPREL: u64 = 23;
    const DT_RELR: u64 = 36;
    const DT_VERSYM: u64 = 0x6fff_fff0;
    const DT_VERDEF: u64 = 0x6fff_fffc;
    let (start, len) = dynamic.ok_or("no dynamic segment")?;
    let (mut hash, mut strtab, mut symtab, mut versions) = (None, None, None, 0);
    for at in (start..start + len).step_by(16) {
        let (tag, value) = (elf.u64(at)?, elf.u64(at + 8)?);
        match tag {
            0 => break,
            DT_NEEDED | DT_RELA | DT_REL | DT_TEXTREL | DT_JMPREL | DT_RELR => {
                return Err(format!("dynamic tag {tag} asks the loader for work"));
            }
            DT_HASH => hash = Some(value),
            DT_STRTAB => strtab = Some(value),
            DT_SYMTAB => symtab = Some(value),
            DT_VERSYM | DT_VERDEF => versions += 1,
            _ => {}
        }
    }
    if versions != 2 {
        return Err("no symbol versions".into());
    }
    let (hash, strtab, symtab) =
        (hash.ok_or("no DT_HASH")?, strtab.ok_or("no DT_STRTAB")?, symtab.ok_or("no DT_SYMTAB")?);
    let count = elf.u32(hash + 4)?;
    let mut exported = Vec::new();
    for index in 1..u64::from(count) {
        let at = symtab + index * 24;
        exported.push(elf.string(strtab + u64::from(elf.u32(at)?))?);
    }
    exported.sort_unstable();
    let mut wanted: Vec<&str> = EXPORTS.to_vec();
    wanted.sort_unstable();
    if exported != wanted {
        return Err(format!("it exports {exported:?}, not {wanted:?}"));
    }
    Ok(())
}

/// An ELF file's bytes, read at offsets that may be out of range.
struct Elf<'a> {
    bytes: &'a [u8],
}

impl Elf<'_> {
    fn field<const N: usize>(&self, at: u64) -> Result<[u8; N], String> {
        let at = usize::try_from(at).map_err(|_| "an offset out of range")?;
        self.bytes
            .get(at..at + N)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| format!("offset {at} is past the end"))
    }

    fn u16(&self, at: u64) -> Result<u16, String> {
        self.field(at).map(u16::from_le_bytes)
    }

    fn u32(&self, at: u64) -> Result<u32, String> {
        self.field(at).map(u32::from_le_bytes)
    }

    fn u64(&self, at: u64) -> Result<u64, String> {
        self.field(at).map(u64::from_le_bytes)
    }

    /// The NUL-terminated string at `at`.
    fn string(&self, at: u64) -> Result<&str, String> {
        let at = usize::try_from(at).map_err(|_| "an offset out of range")?;
        let rest = self.bytes.get(at..).ok_or("a string past the end")?;
        let end = rest.iter().position(|&b| b == 0).ok_or("a string with no end")?;
        std::str::from_utf8(&rest[..end]).map_err(|_| "a name that is not UTF-8".into())
    }
}
