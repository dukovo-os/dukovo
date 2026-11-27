// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `exec` (Phase 8, radix-process(2)): runs a program from a file in a
//! process, in a new address space.
//!
//! The program is read from its file a page at a time, never whole: its
//! headers first (`radix_elf::read_program`), then each loadable segment
//! is mapped as anonymous memory filled from the file when a page is
//! first touched, so a program starts as fast whatever its size, and only
//! the pages it uses take memory. A program that names an interpreter
//! (`PT_INTERP`) gets it mapped too, at a random place in the loader's
//! window, and starts there; a script that starts with `#!` gets its
//! interpreter run with the script's path as an argument (one level: the
//! interpreter must be a program). Both are found from the process's
//! SLOT_ROOT. A position-independent program without an interpreter is
//! placed at random in the image window and its `R_X86_64_RELATIVE`
//! relocations applied here; it may apply any others itself.
//!
//! The stack is laid out as the System V ABI says (`radix_posix::exec`),
//! below a random top, with 8 MiB of room made as it is touched; the
//! start-up page (radix-startup(2)) is mapped read-only at
//! STARTUP_ADDRESS, as for a program the kernel's loader starts, and the
//! auxiliary vector points at it.
//!
//! Everything that can fail is done in the new space, which nothing runs
//! in yet, before the process is touched. Then, on the caller's own
//! process, its other threads are stopped, the new space takes the old
//! one's place (the original capability in the hidden slot 0 is made
//! again, so every capability to the old space goes), the slots with
//! close_on_exec are emptied, handled signals go back to their defaults,
//! a thread starts in the new program, and the calling thread exits. On a
//! process `create` made, which has not started, the process instead
//! joins the tree as the caller's child (`posix_spawn`'s way), and the
//! call returns.
//!
//! Since Phase 18 the program says which system it is written for (ADR
//! 0610): one that names Dukovo's dynamic linker or carries Dukovo's ELF
//! note is Dukovo's, and any other is a Linux program, whose system calls
//! go to the Linux table (`user::linux`). A Linux program gets no start-up
//! page, an auxiliary vector as Linux gives one, and a break past the end
//! of its image.

use alloc::boxed::Box;
use alloc::vec::Vec;

use radix_abi::address_space::{ANONYMOUS_TOP, PAGE_SIZE, USER_START};
use radix_abi::capability::SlotFlags;
use radix_abi::errors::Kernel;
use radix_abi::process::{self, EXEC_MAX, EXIT_STOPPED, LimitKind, ProcessState};
use radix_abi::startup::{self, Startup};
use radix_abi::{Status, UserBuffer};
use radix_cap::{Derivation, ObjectKind, Rights, SlotRef};
use radix_elf::{Abi, Access, DUKOVO_LOADER, ElfError, Kind, NOTE_MAX, Program, marks_dukovo};
use radix_hal::{UserAccess, UserMode};
use radix_klog::warn;
use radix_mm::Kmalloc;
use radix_posix::exec::{Block, BlockError, Stack, aux, shebang};
use radix_vfs::mount::{Location, OpenFlags};
use radix_vfs::{NodeRef, SliceMut, want};

use super::{kmalloc, with_tree};
use crate::fs::{self, FileRef};
use crate::sched::{self, Name, wait};
use crate::user::memory::{Anon, FileSource, Placement, ProgramPages, SpaceObject, SpaceRef};
use crate::user::methods::{Answer, answer};
use crate::user::names::Identity;
use crate::user::objects::{self, Object};
use crate::user::process::{ARGS_KEPT, ProcessObject, ProcessRef, Start};
use crate::user::{Result, UserRef, cap_error, error};
use crate::{Arch, caps};

/// The most bytes of a file read to find its program headers.
const HEAD_MAX: usize = 65536;

/// The first bytes of a file looked at: enough for a `#!` line and, for
/// nearly every program, its program headers.
const HEAD: usize = 4096;

/// A buffer of `len` zero bytes from the kernel's heap.
pub(super) fn zeroed(len: usize) -> Result<Vec<u8, Kmalloc>> {
    radix_mm::bytes::zeroed(len, kmalloc()).map_err(|_| error(Kernel::NoMemory))
}

/// Copies `bytes` to the kernel's heap.
pub(super) fn copied(bytes: &[u8]) -> Result<Vec<u8, Kmalloc>> {
    let mut copy = zeroed(bytes.len())?;
    copy.copy_from_slice(bytes);
    Ok(copy)
}

/// What a refused program becomes.
fn not_executable(err: ElfError) -> Status {
    Kernel::NotExecutable.status(err as u32)
}

/// Reads `out.len()` bytes of `node` from `offset`, all of them.
fn read_exact(node: &NodeRef, offset: u64, out: &mut [u8]) -> Result<()> {
    let mut done = 0;
    while done < out.len() {
        let read =
            node.read(offset + done as u64, &mut SliceMut(&mut out[done..])).map_err(fs::status)?;
        if read == 0 {
            return Err(not_executable(ElfError::Truncated));
        }
        done += read;
    }
    Ok(())
}

/// A program's file: where it is, its size and its first bytes.
struct File {
    node: NodeRef,
    size: u64,
    head: Vec<u8, Kmalloc>,
}

impl File {
    /// Reads the first bytes of the file at `at`, which must be a regular
    /// file: as many as hold an ELF file's program headers, up to
    /// [`HEAD_MAX`].
    fn open(at: &Location) -> Result<File> {
        let node = at.node.clone();
        let stat = node.stat();
        match stat.kind {
            radix_vfs::Kind::File => {}
            radix_vfs::Kind::Directory => {
                return Err(radix_abi::errors::File::IsDirectory.status(0));
            }
            _ => return Err(error(Kernel::NotExecutable)),
        }
        let mut len = (stat.size as usize).min(HEAD);
        let mut head = zeroed(len)?;
        read_exact(&node, 0, &mut head)?;
        if head.starts_with(b"\x7fELF") && head.len() >= 64 {
            let phoff = u64::from_le_bytes(head[32..40].try_into().unwrap_or([0; 8]));
            let phnum = u64::from(u16::from_le_bytes([head[56], head[57]]));
            let end = phoff.saturating_add(phnum * Program::HEADER_SIZE);
            if end > len as u64 && end <= (HEAD_MAX as u64).min(stat.size) {
                len = end as usize;
                head = zeroed(len)?;
                read_exact(&node, 0, &mut head)?;
            }
        }
        Ok(File { node, size: stat.size, head })
    }

    /// Reads it as a program, kept on the heap: two of them would not fit
    /// in `exec`'s stack frame.
    #[inline(never)]
    fn program(&self) -> Result<Box<Program, Kmalloc>> {
        let program = radix_elf::read_program(&self.head, self.size).map_err(not_executable)?;
        Box::try_new_in(program, kmalloc()).map_err(|_| error(Kernel::NoMemory))
    }
}

/// Which system `program`, read from `file`, is written for (ADR 0610):
/// Dukovo's if it names Dukovo's dynamic linker (`interpreter`, its path
/// without the NUL) or one of its note segments carries Dukovo's note,
/// and Linux's otherwise. A note in the first bytes, where linkers put
/// them, is read from those; others from the file.
#[inline(never)]
fn abi_of(file: &File, program: &Program, interpreter: Option<&[u8]>) -> Result<Abi> {
    if interpreter == Some(DUKOVO_LOADER) {
        return Ok(Abi::Dukovo);
    }
    let mut read = None;
    for note in program.notes() {
        let len = note.size.min(NOTE_MAX) as usize;
        let start = note.offset as usize;
        let bytes = match file.head.get(start..start.saturating_add(len)) {
            Some(bytes) => bytes,
            None => {
                let buffer = match &mut read {
                    Some(buffer) => buffer,
                    None => read.insert(zeroed(NOTE_MAX as usize)?),
                };
                read_exact(&file.node, note.offset, &mut buffer[..len])?;
                &buffer[..len]
            }
        };
        if marks_dukovo(bytes, note.align) {
            return Ok(Abi::Dukovo);
        }
    }
    Ok(Abi::of(interpreter, false))
}

/// The directory a process finds interpreters from, its SLOT_ROOT, and
/// whether its capability has the bypass right.
fn root_of(process: &ProcessObject) -> Result<(FileRef, bool)> {
    let (cap, root) = objects::directory(process.cspace(), startup::SLOT_ROOT, Rights::READ)
        .map_err(|_| radix_abi::errors::File::NotFound.status(0))?;
    Ok((root, cap.rights().contains(Rights::BYPASS)))
}

/// Opens the file at `path` from `process`'s root, which its root's name
/// must be able to run: an interpreter's path, absolute or not.
fn open_from(process: &ProcessObject, path: &[u8]) -> Result<File> {
    let mut path = path;
    while let [b'/', rest @ ..] = path {
        path = rest;
    }
    let path = radix_vfs::path::parse(path).map_err(fs::status)?;
    let (root, bypass) = root_of(process)?;
    let vfs = fs::vfs().ok_or(error(Kernel::Gone))?;
    let flags = OpenFlags { want: want::EXECUTE, ..OpenFlags::default() };
    // A root whose user was ended runs nothing more (ADR 0220).
    let who = root.name.who(bypass).ok_or_else(|| fs::status(radix_vfs::Error::Denied))?;
    let at = vfs.open(&who, &root.at, path, flags).map_err(fs::status)?;
    File::open(&at)
}

/// Reads the start of the file at `path` beneath `process`'s root into
/// `out`, as the name the process acts for may read it, and returns how
/// many bytes it read: how the Linux table's `uname` reads the machine's
/// name, from the file Dukovo's C library reads it from (ADR 0611).
pub(in crate::user) fn read_from_root(
    process: &ProcessObject,
    path: &[u8],
    out: &mut [u8],
) -> Result<usize> {
    let path = radix_vfs::path::parse(path).map_err(fs::status)?;
    let (root, bypass) = root_of(process)?;
    let vfs = fs::vfs().ok_or(error(Kernel::Gone))?;
    let flags = OpenFlags { want: want::READ, ..OpenFlags::default() };
    let who = root.name.who(bypass).ok_or_else(|| fs::status(radix_vfs::Error::Denied))?;
    let at = vfs.open(&who, &root.at, path, flags).map_err(fs::status)?;
    // Only a regular file: a device or a pipe there could make the caller
    // wait.
    if at.node.stat().kind != radix_vfs::Kind::File {
        return Err(error(Kernel::InvalidArgument));
    }
    at.node.read(0, &mut SliceMut(out)).map_err(fs::status)
}

/// Where a program was mapped.
struct Mapped {
    entry: u64,
    bias: u64,
    headers: Option<u64>,
}

/// Maps `program`, from `file`, into `space` at a place in `window` (its
/// own place, for a fixed one). Its parts that run are named `name`, for
/// crash records (ADR 0530).
#[inline(never)]
fn map_program(
    space: &SpaceObject,
    file: &File,
    program: &Program,
    window: core::ops::Range<u64>,
    name: Option<&fs::MapName>,
) -> Result<Mapped> {
    let image = &program.image;
    let bias = image
        .random_bias(window.clone(), crate::user::random::place_bits())
        .map_err(not_executable)?;
    let placed = image.place(bias, window).map_err(not_executable)?;
    // The file's pages every process that runs it shares, if it cannot
    // change (ADR 0563).
    let kept = ProgramPages::of(&file.node);
    for segment in image.segments() {
        let access = match segment.access {
            Access::Read => UserAccess::Read,
            Access::ReadWrite => UserAccess::ReadWrite,
            Access::ReadExecute => UserAccess::ReadExecute,
        };
        let page_offset = segment.offset % PAGE_SIZE;
        let source = (segment.filesz > 0).then(|| FileSource {
            node: file.node.clone(),
            base: segment.offset - page_offset,
            size: page_offset + segment.filesz,
            name: name.filter(|_| access == UserAccess::ReadExecute).cloned(),
            kept: kept.clone(),
        });
        let anon = Anon { access: Some(access), file: source, ..Anon::default() };
        let at = segment.first_page().wrapping_add(bias);
        space
            .map_anonymous(segment.pages(), anon, Placement::FixedNoReplace(at))
            .map_err(|_| not_executable(ElfError::OutOfRange))?;
    }
    Ok(Mapped {
        entry: placed.entry(),
        bias,
        headers: program.headers.map(|at| at.wrapping_add(bias)),
    })
}

/// Applies the relocations of `program`, a position-independent program
/// without an interpreter mapped into `space` moved by `bias`: those the
/// kernel can (`R_X86_64_RELATIVE`), leaving any others to the program.
#[inline(never)]
fn relocate(space: &SpaceObject, file: &File, program: &Program, bias: u64) -> Result<()> {
    let Some((offset, size)) = program.dynamic else { return Ok(()) };
    let mut table = zeroed(size.min(4096) as usize)?;
    read_exact(&file.node, offset, &mut table)?;
    // Relocations this loader does not understand are the program's own.
    let Ok((start, count)) = program.relocations(&table) else { return Ok(()) };
    let placed = program.image.place(bias, 0..u64::MAX).map_err(not_executable)?;
    let entry_size = radix_elf::Placed::RELOCATION_SIZE;
    let mut chunk = zeroed(4096 / entry_size as usize * entry_size as usize)?;
    let per_chunk = chunk.len() as u64 / entry_size;
    // The page the last word went to, and its frame: most pages take
    // many words, and a look at the page tables for each costs more than
    // the write.
    let mut last: Option<(u64, u64)> = None;
    let mut done = 0;
    while done < count {
        let n = (count - done).min(per_chunk);
        let bytes = &mut chunk[..(n * entry_size) as usize];
        read_exact(&file.node, start + done * entry_size, bytes)?;
        for entry in bytes.chunks_exact(entry_size as usize) {
            let Ok(Some((address, value))) = placed.relocation(entry) else { continue };
            let (page, offset) = (address & !(PAGE_SIZE - 1), address % PAGE_SIZE);
            if offset > PAGE_SIZE - 8 {
                space.write_new(address, &value.to_le_bytes())?;
                continue;
            }
            let phys = match last {
                Some((at, phys)) if at == page => phys,
                _ => last.insert((page, space.touch(page)?)).1,
            };
            Arch::write_frames(phys + offset, &value.to_le_bytes());
        }
        done += n;
    }
    Ok(())
}

/// Maps the start-up page at STARTUP_ADDRESS, read-only, filled in for a
/// program moved by `bias`.
#[inline(never)]
fn map_startup(space: &SpaceObject, bias: u64) -> Result<()> {
    let anon = Anon { access: Some(UserAccess::Read), ..Anon::default() };
    space
        .map_anonymous(1, anon, Placement::FixedNoReplace(startup::STARTUP_ADDRESS))
        .map_err(|_| not_executable(ElfError::OutOfRange))?;
    let mut page = [0u8; Startup::SIZE];
    let mut put =
        |offset: usize, value: &[u8]| page[offset..offset + value.len()].copy_from_slice(value);
    put(Startup::OFFSET_VERSION, &startup::STARTUP_VERSION.to_le_bytes());
    put(Startup::OFFSET_CPU, &startup::ANY_CPU.to_le_bytes());
    put(Startup::OFFSET_TSC_HZ, &Arch::cycle_counter_hz().to_le_bytes());
    put(Startup::OFFSET_IMAGE_BIAS, &bias.to_le_bytes());
    put(Startup::OFFSET_ARGS_OFFSET, &(Startup::SIZE as u32).to_le_bytes());
    space.write_new(startup::STARTUP_ADDRESS, &page)
}

/// The block `exec` was given, copied in.
fn copy_block(buffer: UserBuffer) -> Result<Vec<u8, Kmalloc>> {
    if buffer.len > EXEC_MAX {
        return Err(error(Kernel::ArgumentsTooLong));
    }
    if !radix_ipc::range::user_bytes(buffer.address, buffer.len) {
        return Err(error(Kernel::BadAddress));
    }
    let mut bytes = zeroed(buffer.len as usize)?;
    if !Arch::copy_from_user(&mut bytes, buffer.address) {
        return Err(error(Kernel::BadAddress));
    }
    Ok(bytes)
}

/// `opened`, shared, if there is memory for it.
fn named(opened: fs::Opened) -> Option<fs::MapName> {
    alloc::sync::Arc::try_new_in(opened, kmalloc()).ok()
}

/// The last part of a path: what a process run from it is named.
fn base_name(path: &[u8]) -> &[u8] {
    let path = path.strip_suffix(b"/").unwrap_or(path);
    path.rsplit(|&b| b == b'/').next().unwrap_or(path)
}

/// `exec`, called by `user`'s thread as method `label` on `target` (its
/// own process, or one `create` made that has not started), whose
/// capability is in the caller's slot `slot`: runs the program in the
/// file in the caller's slot `file`, with the strings in `block`.
#[inline(never)]
pub fn exec(
    user: &UserRef,
    target: &ProcessRef,
    slot: u32,
    label: u32,
    file: u32,
    block: UserBuffer,
) -> Result<Answer> {
    exec_from(user, target, slot, label, file, Strings::Caller(block))
}

/// Where `exec`'s block of strings is.
pub enum Strings {
    /// In the caller's memory.
    Caller(UserBuffer),
    /// In the kernel's: the Linux table's `execve` (ADR 0611) builds it.
    Given(Vec<u8, Kmalloc>),
}

/// [`exec`], with the block where `block` says.
#[inline(never)]
pub fn exec_from(
    user: &UserRef,
    target: &ProcessRef,
    slot: u32,
    label: u32,
    file: u32,
    block: Strings,
) -> Result<Answer> {
    let own = ProcessRef::ptr_eq(target, &user.process);
    if !own && target.state().0 != ProcessState::Created {
        return Err(Kernel::InvalidArgument.status(0));
    }
    let (cap, file) = objects::file(user.process.cspace(), file, Rights::READ)?;
    // The name the file was opened for must be able to run it.
    let who = file.name.who(cap.rights().contains(Rights::BYPASS));
    let who = who.ok_or_else(|| fs::status(radix_vfs::Error::Denied))?;
    who.check(&*file.at.node, want::EXECUTE).map_err(fs::status)?;
    let bytes = match block {
        Strings::Caller(buffer) => copy_block(buffer)?,
        Strings::Given(bytes) => bytes,
    };
    let block = Block::read(&bytes).map_err(|err| match err {
        BlockError::TooLong => error(Kernel::ArgumentsTooLong),
        BlockError::Malformed => Kernel::InvalidArgument.status(1),
    })?;
    let mut extra = block.extra();
    let cwd = extra.next();
    let execfn = extra.next();

    // The program, or a script and its interpreter. The firewall knows
    // it by the file run, the script's for a script (ADR 0334).
    let ran = crate::net::firewall::Program { mount: file.at.mount.id, id: file.at.node.stat().id };
    let mut program_file = File::open(&file.at)?;
    // The path of the file that runs, for crash records (ADR 0530): none
    // if there is no memory for it, which costs only its name there.
    let mut image = named(file.opened);
    drop(file);
    let mut script = None;
    if let Some(line) = shebang(&program_file.head) {
        let interpreter = copied(line.interpreter)?;
        let argument = line.argument.map(copied).transpose()?;
        program_file = open_from(target, &interpreter)?;
        if program_file.head.starts_with(b"#!") {
            return Err(error(Kernel::NotExecutable));
        }
        image = named(fs::Opened::from_root(&interpreter));
        script = Some((interpreter, argument));
    }
    let program = program_file.program()?;
    let interpreter_path = match program.interpreter {
        Some((offset, len)) => {
            let mut path = zeroed(len as usize)?;
            read_exact(&program_file.node, offset, &mut path)?;
            let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
            path.truncate(end);
            Some(path)
        }
        None => None,
    };
    let linux = abi_of(&program_file, &program, interpreter_path.as_deref())? == Abi::Linux;
    // A seccomp filter speaks Linux's numbers: a filtered thread may not
    // leave it behind by running a Dukovo program (ADR 0623).
    if !linux && user.linux.seccomp.is_on() {
        return Err(Kernel::MissingRights.status(0));
    }
    let interpreter = match &interpreter_path {
        Some(path) => {
            let file = open_from(target, path)?;
            let found = file.program()?;
            if found.image.kind != Kind::Relocatable || found.interpreter.is_some() {
                return Err(error(Kernel::NotExecutable));
            }
            Some((file, found, named(fs::Opened::from_root(path))))
        }
        None => None,
    };

    // The new address space, filled.
    let domain = target.account().domain().clone();
    let space = SpaceObject::new(Some(domain)).ok_or(error(Kernel::NoMemory))?;
    let window = match program.image.kind {
        Kind::Fixed => USER_START + PAGE_SIZE..ANONYMOUS_TOP / 2,
        Kind::Relocatable => startup::IMAGE_BASE..startup::IMAGE_BASE + startup::WINDOW,
    };
    let main = map_program(&space, &program_file, &program, window, image.as_ref())?;
    let loader = match &interpreter {
        Some((file, found, name)) => {
            let window = startup::LOADER_BASE..startup::LOADER_BASE + startup::WINDOW;
            Some(map_program(&space, file, found, window, name.as_ref())?)
        }
        None => None,
    };
    if program.image.kind == Kind::Relocatable && interpreter.is_none() {
        relocate(&space, &program_file, &program, main.bias)?;
    }
    // A Linux program's break starts past its image, a random number of
    // pages on, as Linux places it; a Dukovo program has none, and has
    // the start-up page.
    let brk = if linux {
        let end = program.image.span().end.wrapping_add(main.bias).next_multiple_of(PAGE_SIZE);
        end + crate::user::random::place(BREAK_WINDOW / PAGE_SIZE) * PAGE_SIZE
    } else {
        map_startup(&space, main.bias)?;
        0
    };

    // Every program reads the clocks through the vDSO (ADR 0616).
    let vdso = crate::user::vdso::map(&space)?;

    // The room RLIMIT_STACK gives it, made as it is touched.
    let stack_bytes = super::limits::stack_bytes(&user.process.posix.lock().limits, PAGE_SIZE);
    let sp = build_stack(
        &space,
        &block,
        script.as_ref(),
        (cwd, execfn),
        &program,
        (&main, loader.as_ref(), vdso),
        stack_bytes,
        linux.then(|| target.identity()).as_ref(),
    )?;
    space.set_stack(sp);
    let entry = loader.as_ref().map_or(main.entry, |loader| loader.entry);
    let run_as = execfn.or(block.args().next()).unwrap_or(b"");
    let name = base_name(run_as);
    let name = core::str::from_utf8(&name[..name.len().min(process::NAME_MAX as usize)])
        .unwrap_or("program");
    let name = Name::new(name);
    keep_args(target, &block, script.as_ref(), execfn);
    let old_image = {
        let mut posix = target.posix.lock();
        posix.program = ran;
        core::mem::replace(&mut posix.image, image)
    };
    drop(old_image);
    if let Some(cwd) = cwd {
        super::set_directory(target, cwd);
    }

    let start = Begin { space: &space, at: [entry, sp, 0], linux, brk };
    if own {
        replace_own(user, target, &start, name, run_as)
    } else {
        start_created(user, target, slot, &start, name, run_as)?;
        answer(label, 0, [0; 8])
    }
}

/// How far past a Linux program's image its break may start: 32 MiB, as
/// Linux randomizes it.
const BREAK_WINDOW: u64 = 32 << 20;

/// How the new program begins: its address space, where its thread
/// starts, whether it is a Linux program and, for one, its break.
struct Begin<'a> {
    space: &'a SpaceRef,
    at: [u64; 3],
    linux: bool,
    brk: u64,
}

/// A script's interpreter, and the argument its `#!` line gives it.
type Script = (Vec<u8, Kmalloc>, Option<Vec<u8, Kmalloc>>);

/// Has `target` keep the arguments its new program starts with, as
/// [`build_stack`] lays them out, for `/processes/<n>/cmdline`: for a script,
/// its interpreter, the `#!` line's argument and the script's path in
/// place of the first of the block's; each ends with a NUL.
fn keep_args(
    target: &ProcessRef,
    block: &Block<'_>,
    script: Option<&Script>,
    execfn: Option<&[u8]>,
) {
    let mut kept: Vec<u8, Kmalloc> = Vec::new_in(kmalloc());
    if kept.try_reserve_exact(ARGS_KEPT).is_err() {
        return;
    }
    let mut add = |arg: &[u8]| {
        for &byte in arg.iter().chain([0u8].iter()) {
            if kept.push_within_capacity(byte).is_err() {
                return;
            }
        }
    };
    let mut given = block.args();
    let first = given.next();
    if let Some((interpreter, argument)) = script {
        add(interpreter);
        if let Some(argument) = argument {
            add(argument);
        }
        add(execfn.or(first).unwrap_or(b""));
    } else if let Some(first) = first {
        add(first);
    }
    for arg in given {
        add(arg);
    }
    target.set_args(&kept);
}

/// Lays out the new program's stack in `space` as the System V ABI says:
/// its arguments (the script's interpreter's first, for a script), its
/// environment and its auxiliary vector, below a random top with
/// `stack_bytes` of room, of which they may take a quarter, as on Linux.
/// The vector names the vDSO, at `vdso`, if the program has one. Returns
/// the stack pointer it starts with.
///
/// A Linux program (`linux`, with the name its process acts for) is told
/// its user and group, the CPU's features and the most a signal's frame
/// takes, as Linux tells a program, and nothing of Radix's: no start-up
/// page and no working directory, which the kernel keeps for it.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn build_stack(
    space: &SpaceObject,
    block: &Block<'_>,
    script: Option<&Script>,
    (cwd, execfn): (Option<&[u8]>, Option<&[u8]>),
    program: &Program,
    (main, loader, vdso): (&Mapped, Option<&Mapped>, Option<u64>),
    stack_bytes: u64,
    linux: Option<&Identity>,
) -> Result<u64> {
    let mut args: Vec<&[u8], Kmalloc> = Vec::new_in(kmalloc());
    let mut env: Vec<&[u8], Kmalloc> = Vec::new_in(kmalloc());
    let count = block.arg_count() as usize + 3;
    args.try_reserve_exact(count).map_err(|_| error(Kernel::NoMemory))?;
    env.try_reserve_exact(block.env_count() as usize).map_err(|_| error(Kernel::NoMemory))?;
    let mut given = block.args();
    let first = given.next();
    let run_as = execfn.or(first).unwrap_or(b"");
    if let Some((interpreter, argument)) = script {
        // `#!interpreter argument`, then the script, then its arguments.
        let _ = args.push_within_capacity(&interpreter[..]);
        if let Some(argument) = argument {
            let _ = args.push_within_capacity(&argument[..]);
        }
        let _ = args.push_within_capacity(run_as);
    } else if let Some(first) = first {
        let _ = args.push_within_capacity(first);
    }
    for arg in given {
        let _ = args.push_within_capacity(arg);
    }
    for var in block.env() {
        let _ = env.push_within_capacity(var);
    }
    let (user, group) = linux.map_or((0, 0), |name| (name.user, name.group));
    let common = [
        (aux::AT_PHDR, main.headers.unwrap_or(0)),
        (aux::AT_PHENT, Program::HEADER_SIZE),
        (aux::AT_PHNUM, u64::from(program.header_count)),
        (aux::AT_PAGESZ, PAGE_SIZE),
        (aux::AT_BASE, loader.map_or(0, |loader| loader.bias)),
        (aux::AT_FLAGS, 0),
        (aux::AT_ENTRY, main.entry),
        (aux::AT_UID, u64::from(user)),
        (aux::AT_EUID, u64::from(user)),
        (aux::AT_GID, u64::from(group)),
        (aux::AT_EGID, u64::from(group)),
        (aux::AT_CLKTCK, 100),
        (aux::AT_SECURE, 0),
    ];
    let own = if linux.is_some() {
        [
            (aux::AT_HWCAP, Arch::hardware_capabilities()),
            (aux::AT_HWCAP2, 0),
            (aux::AT_MINSIGSTKSZ, Arch::signal_frame_size()),
        ]
    } else {
        [
            (aux::AT_HWCAP, 0),
            (aux::AT_RADIX_STARTUP, startup::STARTUP_ADDRESS),
            // Its place in the vector, taken by nothing.
            (aux::AT_IGNORE, 0),
        ]
    };
    let vdso = vdso.map_or((aux::AT_IGNORE, 0), |at| (aux::AT_SYSINFO_EHDR, at));
    let mut auxv = [(0, 0); 17];
    auxv[..common.len()].copy_from_slice(&common);
    auxv[common.len()..common.len() + own.len()].copy_from_slice(&own);
    auxv[common.len() + own.len()] = vdso;
    let mut random = [0u8; 16];
    crate::user::random::fill(&mut random);
    let stack = Stack {
        args: &args,
        env: &env,
        aux: &auxv,
        execfn: run_as,
        // A Linux program's working directory is the kernel's to keep.
        cwd: cwd.filter(|_| linux.is_none()),
        platform: b"x86_64",
        random,
    };
    let room = stack.size().next_multiple_of(16);
    if room > stack_bytes / 4 {
        return Err(error(Kernel::ArgumentsTooLong));
    }
    let windows = startup::STACK_WINDOW / PAGE_SIZE - stack_bytes / PAGE_SIZE;
    let top = startup::STACK_TOP - crate::user::random::place(windows) * PAGE_SIZE;
    let anon = Anon { access: Some(UserAccess::ReadWrite), ..Anon::default() };
    space
        .map_anonymous(stack_bytes / PAGE_SIZE, anon, Placement::FixedNoReplace(top - stack_bytes))
        .map_err(|_| error(Kernel::NoMemory))?;
    let mut out = zeroed(room as usize)?;
    let sp = stack.write(top, &mut out).map_err(|_| error(Kernel::ArgumentsTooLong))?;
    space.write_new(top - room, &out)?;
    Ok(sp)
}

/// Makes `space` the address space of `process`: its hidden slot 0 holds
/// a new original capability to it, and SLOT_SPACE one derived from that;
/// the slots with close_on_exec are emptied. With `thread`, that slot is
/// emptied too, for the new program's thread.
fn install_space(process: &ProcessRef, space: &SpaceRef, empty_thread: bool) -> Result<()> {
    let kept = SlotRef::new(process.keep_space(), 0);
    // Every capability to the old space goes with its original.
    caps::revoke(kept).map_err(cap_error)?;
    let cspace = process.cspace();
    let mask = SlotFlags::CLOSE_ON_EXEC.0 as u8;
    if process.has_record_locks() {
        // Closing a descriptor of a file lets go of the process's record
        // locks on it, as `capability.delete` does.
        let mut from = 0;
        let flagged = |from| caps::store().and_then(|store| store.find_flagged(cspace, mask, from));
        while let Ok(Some(slot)) = flagged(from) {
            if let Ok(cap) = objects::capability(cspace, slot)
                && let Ok(Object::File(file) | Object::Directory(file)) = objects::object_of(&cap)
            {
                fs::locks::process_gone(process.number(), Some(&file.at.node));
            }
            from = slot + 1;
        }
    }
    // One change after another: the process's other threads have stopped,
    // and nobody else changes its spaces. What the deletes release is
    // finished once, at the end.
    let installed = (|| -> Result<()> {
        let store = caps::store().map_err(cap_error)?;
        store.delete(kept, &mut caps::release).map_err(cap_error)?;
        let rights = ObjectKind::AddressSpace.rights();
        let mut object = Some(Object::Space(space.clone()));
        objects::create(kept, ObjectKind::AddressSpace, rights, &mut object, None)?;
        let at = SlotRef::new(cspace, startup::SLOT_SPACE);
        let _ = store.delete(at, &mut caps::release);
        let how = Derivation { rights, ..Derivation::COPY };
        store.derive(kept, at, how).map_err(cap_error)?;
        let mut from = 0;
        while let Some(slot) = store.find_flagged(cspace, mask, from).map_err(cap_error)? {
            let _ = store.delete(SlotRef::new(cspace, slot), &mut caps::release);
            from = slot + 1;
        }
        if empty_thread {
            let _ = store.delete(SlotRef::new(cspace, startup::SLOT_THREAD), &mut caps::release);
        }
        Ok(())
    })();
    caps::finish();
    installed?;
    process.set_space(space.clone());
    Ok(())
}

/// The caller's own process runs the new program, at `path` as the C
/// library named it: its other threads stop, `space` takes the old one's
/// place, and a thread starts at `start`; the calling thread then exits.
fn replace_own(
    user: &UserRef,
    process: &ProcessRef,
    start: &Begin<'_>,
    name: Name,
    path: &[u8],
) -> Result<Answer> {
    let me = sched::current().ok_or(error(Kernel::Gone))?;
    // The new program's empty set of pending signals, made while a lack of
    // memory can still go back to the old program.
    let fresh = super::new_pending().ok_or(error(Kernel::NoMemory))?;
    // The other threads stop before anything changes: none of them may see
    // the old program's capabilities after the new program starts.
    process.stop_other_threads(&me);
    wait::wait_until(process.threads_key(), "exec", || {
        process.thread_count() <= 1 || me.should_stop()
    });
    if me.should_stop() {
        return Err(super::signal::interrupted(super::signal::Restart::Never));
    }
    // Its parent may no longer move it to another group (EACCES) once a
    // descriptor that closes on exec has closed, which a parent can wait
    // for: mark it before they close.
    with_tree(|tree| {
        if let Some(member) = tree.get_mut(process.number()) {
            member.execed = true;
        }
    });
    // The robust locks a Linux thread holds are marked as its owner's,
    // in the old program's memory, which the thread leaves now (ADR 0617).
    crate::user::linux::ending(&me, user);
    if let Err(err) = install_space(process, start.space, true) {
        // Past saving: the old program's capabilities may be gone.
        warn!("User: process {} could not run its new program: {:#x}", process.number(), err.0);
        process.posix.lock().ended_by.get_or_insert(radix_abi::signal::SIGKILL);
        process.exit(EXIT_STOPPED);
        return Ok(Answer::Exit(EXIT_STOPPED));
    }
    {
        // Its POSIX timers go, and their signals with them.
        let mut posix = process.posix.lock();
        super::timer::delete_all(process, &mut posix);
    }
    let (mask, mut pending) = {
        let mut signals = user.signals.lock();
        let pending = core::mem::replace(&mut signals.pending, fresh);
        (signals.mask, pending)
    };
    process.posix.lock().actions.reset_for_exec();
    // No thread of the old program is left to make a call of the system
    // it was written for.
    process.linux.start(start.linux, start.brk);
    // A parent waiting since a `vfork` goes on (`linux::process`).
    wait::wake_all(process.threads_key());
    process.set_name(name);
    crate::fs::trace::exec(process, me.id(), path);
    // The new program's thread has the process's number, as on Linux,
    // whichever thread called exec; the caller, which has it if it was the
    // first, exits as the new one starts, and gives it its place in the
    // process's domain, so that a supervisor counting the domain's threads
    // never finds two for this one.
    process.number_next_thread();
    let started = ProcessObject::start(
        process,
        Start::Fresh(start.at),
        startup::SLOT_THREAD,
        None,
        Some(&me),
        mask,
        &mut |thread| {
            // The signals pending for the old thread are the new one's.
            if let Some(new) = thread.user() {
                let mut signals = new.signals.lock();
                while let Some((signal, info)) = pending.take(u64::MAX) {
                    signals.pending.add(signal, info);
                }
            }
            Ok(())
        },
    );
    if let Err(err) = started {
        warn!("User: process {} could not start its new program: {:#x}", process.number(), err.0);
        process.posix.lock().ended_by.get_or_insert(radix_abi::signal::SIGKILL);
        process.exit(EXIT_STOPPED);
        return Ok(Answer::Exit(EXIT_STOPPED));
    }
    Ok(Answer::Exit(0))
}

/// A process `create` made, whose capability is in `user`'s slot `slot`,
/// runs the new program, at `path`: it gets `space`, its own process in
/// SLOT_PROCESS if that slot is empty, a place in the tree as the caller's
/// child, and a thread at `start`.
#[allow(clippy::too_many_arguments)]
fn start_created(
    user: &UserRef,
    process: &ProcessRef,
    slot: u32,
    start: &Begin<'_>,
    name: Name,
    path: &[u8],
) -> Result<()> {
    install_space(process, start.space, false)?;
    let me = &user.process;
    let own = SlotRef::new(process.cspace(), startup::SLOT_PROCESS);
    if matches!(caps::get(own), Err(radix_cap::CapError::Empty)) {
        let how = Derivation { rights: Rights::READ.union(Rights::CONTROL), ..Derivation::COPY };
        caps::derive(SlotRef::new(me.cspace(), slot), own, how).map_err(cap_error)?;
    }
    with_tree(|tree| -> Result<()> {
        super::join(tree, me)?;
        tree.insert_child(process.number(), me.number(), process.clone())
            .map_err(super::tree_error)?;
        if let Some(member) = tree.get_mut(process.number()) {
            member.execed = true;
        }
        Ok(())
    })?;
    process.make_posix_child();
    {
        let parent = me.posix.lock();
        // The new process is not in use yet: nothing else takes its lock.
        let mut posix = process.posix.lock_nested(1);
        posix.in_tree = true;
        posix.terminal.clone_from(&parent.terminal);
        posix.inherit(&parent);
        process.set_file_size_limit(posix.limits.get(LimitKind::FileSize).current);
    }
    let (class, level) = me.io_priority();
    process.set_io_priority(class, level);
    process.set_name(name);
    crate::fs::trace::forked(me, process);
    crate::fs::trace::exec(process, process.number(), path);
    process.linux.start(start.linux, start.brk);
    let mask = user.signals.lock().mask;
    ProcessObject::start(
        process,
        Start::Fresh(start.at),
        startup::SLOT_THREAD,
        None,
        None,
        mask,
        &mut |_| Ok(()),
    )?;
    Ok(())
}
