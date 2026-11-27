// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's memory calls (ADR 0610, 0611): each is a method of the
//! process's address space (SLOT_SPACE), as Dukovo's C library calls it.
//!
//! Dukovo's rule that no page is writable and executable at once holds
//! for Linux programs too: `mmap` and `mprotect` refuse PROT_WRITE with
//! PROT_EXEC with EACCES, as Linux does where a security module forbids
//! such memory, but for the private anonymous memory of a process given
//! memory.jit (ADR 0624), where a JIT compiler writes its code. A refusal
//! for want of memory.jit is noted on the process, for `linux run` to say
//! so. `madvise`'s MADV_DONTNEED and MADV_FREE let go of the pages
//! (radix-address-space(2)'s `forget`), so that they read as zeros or as
//! their file again. `mremap` is the address space's `remap` (ADR 0629),
//! which grows a mapping in place or moves its pages without copying
//! them. The break is the process's own (`LinuxProcess`).

use core::cmp::Ordering;

use radix_abi::UserBuffer;
use radix_abi::address_space::{self as abi, MapFlags, PAGE_SIZE, USER_TOP};
use radix_abi::capability::Rights;
use radix_abi::errors::{File, Kernel};
use radix_abi::process::{self, MemoryFileArgs};
use radix_abi::startup::{SLOT_PROCESS, SLOT_SPACE};
use radix_linux::errno::{
    EACCES, EBADF, EFAULT, EINVAL, ENAMETOOLONG, ENODEV, ENOMEM, EOPNOTSUPP, EPERM,
};
use radix_linux::memfd;

use super::{Args, Error, Result, fail, files, is, method, paths, results};
use crate::sched::ThreadRef;
use crate::user::UserRef;

/// `mmap`'s and `mprotect`'s PROT_ bits.
const PROT_READ: u64 = 0x1;
const PROT_WRITE: u64 = 0x2;
const PROT_EXEC: u64 = 0x4;
/// PROT_SEM, PROT_GROWSDOWN and PROT_GROWSUP, which change nothing here.
const PROT_IGNORED: u64 = 0x8 | 0x0100_0000 | 0x0200_0000;

/// `mmap`'s MAP_ flags.
const MAP_TYPE: u64 = 0x3;
const MAP_SHARED: u64 = 0x1;
const MAP_PRIVATE: u64 = 0x2;
const MAP_SHARED_VALIDATE: u64 = 0x3;
const MAP_FIXED: u64 = 0x10;
const MAP_ANONYMOUS: u64 = 0x20;
const MAP_32BIT: u64 = 0x40;
const MAP_HUGETLB: u64 = 0x4_0000;
const MAP_SYNC: u64 = 0x8_0000;
const MAP_FIXED_NOREPLACE: u64 = 0x10_0000;
/// The flags MAP_SHARED_VALIDATE knows; another is EOPNOTSUPP with it.
/// MAP_GROWSDOWN, MAP_DENYWRITE, MAP_EXECUTABLE, MAP_LOCKED,
/// MAP_NORESERVE, MAP_POPULATE, MAP_NONBLOCK and MAP_STACK are among them,
/// and change nothing here: memory is made as it is touched.
const MAP_KNOWN: u64 = MAP_TYPE
    | MAP_FIXED
    | MAP_ANONYMOUS
    | MAP_32BIT
    | 0x100
    | 0x800
    | 0x1000
    | 0x2000
    | 0x4000
    | 0x8000
    | 0x1_0000
    | 0x2_0000
    | MAP_HUGETLB
    | MAP_FIXED_NOREPLACE
    | (0x3f << 26);

/// Whether `prot` asks for memory that is writable and executable.
fn writes_and_executes(prot: u64) -> bool {
    prot & (PROT_WRITE | PROT_EXEC) == PROT_WRITE | PROT_EXEC
}

/// The native rights for `prot`, as Dukovo's C library gives them: write
/// and execute each imply read, as on x86-64. EACCES for writable and
/// executable memory, unless `jit`: the process has memory.jit and the
/// memory is private and anonymous, or may be (`mprotect`, which the
/// address space refuses for any other).
fn rights_of(prot: u64, jit: bool) -> Result<Rights> {
    if writes_and_executes(prot) && !jit {
        return fail(EACCES);
    }
    let mut rights = Rights::NONE;
    for (bit, right) in [
        (PROT_READ, Rights::READ),
        (PROT_WRITE, Rights::READ.union(Rights::WRITE)),
        (PROT_EXEC, Rights::READ.union(Rights::EXECUTE)),
    ] {
        if prot & bit != 0 {
            rights = rights.union(right);
        }
    }
    Ok(rights)
}

/// Whether `address` is on a page boundary.
fn aligned(address: u64) -> bool {
    address.is_multiple_of(PAGE_SIZE)
}

/// `mmap(addr, length, prot, flags, fd, offset)`.
pub fn mmap(me: &ThreadRef, user: &UserRef, a: &Args) -> Result<u64> {
    let [address, length, prot, flags, fd, offset] = *a;
    let flags = flags & u64::from(u32::MAX);
    if !aligned(offset) || length == 0 {
        return fail(EINVAL);
    }
    match flags & MAP_TYPE {
        MAP_SHARED | MAP_PRIVATE => {}
        MAP_SHARED_VALIDATE if flags & !MAP_KNOWN == 0 && flags & MAP_SYNC == 0 => {}
        MAP_SHARED_VALIDATE => return fail(EOPNOTSUPP),
        _ => return fail(EINVAL),
    }
    if length > USER_TOP {
        return fail(ENOMEM);
    }
    let anonymous = flags & MAP_ANONYMOUS != 0;
    // No huge pages are reserved, and nothing is placed below 2 GiB on
    // request: Linux's answers when it has none to give.
    if flags & MAP_HUGETLB != 0 {
        return fail(if anonymous { ENOMEM } else { EINVAL });
    }
    if flags & MAP_32BIT != 0 {
        return fail(ENOMEM);
    }
    let mut map_flags = MapFlags::NONE;
    if flags & MAP_FIXED_NOREPLACE != 0 {
        map_flags = map_flags.union(MapFlags::FIXED_NO_REPLACE);
    } else if flags & MAP_FIXED != 0 {
        map_flags = map_flags.union(MapFlags::FIXED);
    }
    if !map_flags.is_empty() && !aligned(address) {
        return fail(EINVAL);
    }
    if flags & MAP_TYPE != MAP_PRIVATE {
        map_flags = map_flags.union(MapFlags::SHARED);
    }
    let private_anonymous = anonymous && flags & MAP_TYPE == MAP_PRIVATE;
    let jit = user.process.jit() && private_anonymous;
    if private_anonymous && !jit && writes_and_executes(prot) {
        user.process.refuse_jit();
    }
    let rights = rights_of(prot, jit)?;
    let mapped = if anonymous {
        let args = abi::MapAnonymousArgs { address, size: length, rights, flags: map_flags };
        results(me, user, SLOT_SPACE, abi::MAP_ANONYMOUS, args.encode())
    } else {
        let file = paths::slot_of(fd)?;
        let args =
            abi::MapFileArgs { address, size: length, rights, flags: map_flags, file, offset };
        results(me, user, SLOT_SPACE, abi::MAP_FILE, args.encode())
    };
    match mapped {
        Ok(words) => Ok(abi::MapAnonymousResults::decode(&words)?.address),
        // What the descriptor holds is not a file that maps: ENODEV, as
        // Linux says it of a pipe or a directory; nothing at all, EBADF.
        Err(Error::Native(status))
            if is_file_error(status, File::NoDevice)
                || is_file_error(status, File::IsDirectory) =>
        {
            fail(ENODEV)
        }
        Err(Error::Native(status)) if is(status, Kernel::WrongKind) => fail(ENODEV),
        Err(Error::Native(status))
            if is(status, Kernel::EmptySlot) || is(status, Kernel::NoSuchSlot) =>
        {
            fail(EBADF)
        }
        Err(Error::Native(status)) if is(status, Kernel::MissingRights) => fail(EACCES),
        Err(err) => Err(err),
    }
}

/// Whether `status` is the file error `kind`'s, whatever its detail.
fn is_file_error(status: radix_abi::Status, kind: File) -> bool {
    let wanted = kind.status(0);
    status.domain() == wanted.domain() && status.code() == wanted.code()
}

/// `munmap(addr, length)`.
pub fn munmap(me: &ThreadRef, user: &UserRef, address: u64, length: u64) -> Result<u64> {
    if !aligned(address) || length == 0 || length > USER_TOP {
        return fail(EINVAL);
    }
    let args = abi::DiscardArgs { address, size: length };
    results(me, user, SLOT_SPACE, abi::DISCARD, args.encode()).map(|_| 0)
}

/// `mprotect(addr, length, prot)`.
pub fn mprotect(
    me: &ThreadRef,
    user: &UserRef,
    address: u64,
    length: u64,
    prot: u64,
) -> Result<u64> {
    if !aligned(address) || prot & !(PROT_READ | PROT_WRITE | PROT_EXEC | PROT_IGNORED) != 0 {
        return fail(EINVAL);
    }
    if length == 0 {
        return Ok(0);
    }
    if length > USER_TOP {
        return fail(ENOMEM);
    }
    let jit = user.process.jit();
    if !jit && writes_and_executes(prot) {
        user.process.refuse_jit();
    }
    let args = abi::ProtectArgs { address, size: length, rights: rights_of(prot, jit)? };
    match results(me, user, SLOT_SPACE, abi::PROTECT, args.encode()) {
        Ok(_) => Ok(0),
        // A page that is not mapped: ENOMEM, as Linux says it.
        Err(Error::Native(status)) if is(status, Kernel::NotMapped) => fail(ENOMEM),
        // A shared mapping that may only be read, made writable.
        Err(Error::Native(status)) if is(status, Kernel::MissingRights) => fail(EACCES),
        Err(err) => Err(err),
    }
}

/// `madvise`'s advice values.
const MADV_DONTNEED: u64 = 4;
const MADV_FREE: u64 = 8;
const MADV_REMOVE: u64 = 9;
const MADV_WIPEONFORK: u64 = 18;
const MADV_DONTNEED_LOCKED: u64 = 24;
const MADV_HWPOISON: u64 = 100;
const MADV_SOFT_OFFLINE: u64 = 101;

/// `madvise(addr, length, advice)`: MADV_DONTNEED, MADV_DONTNEED_LOCKED
/// and MADV_FREE let go of the pages (`forget`), which then read as Linux
/// says; the advice that only says how the memory will be used is taken
/// and changes nothing; MADV_REMOVE, which punches a hole in shared
/// memory, and MADV_WIPEONFORK, which a child must see as zeros, are
/// EINVAL, as a Linux kernel without them answers, so that a program
/// falls back; the advice that needs privilege is EPERM.
pub fn madvise(
    me: &ThreadRef,
    user: &UserRef,
    address: u64,
    length: u64,
    advice: u64,
) -> Result<u64> {
    if !aligned(address) {
        return fail(EINVAL);
    }
    match advice as i32 as i64 as u64 {
        MADV_DONTNEED | MADV_DONTNEED_LOCKED | MADV_FREE => {
            if length == 0 {
                return Ok(0);
            }
            if length > USER_TOP {
                return fail(EINVAL);
            }
            let args = abi::ForgetArgs { address, size: length };
            match results(me, user, SLOT_SPACE, abi::FORGET, args.encode()) {
                Ok(_) => Ok(0),
                Err(Error::Native(status)) if is(status, Kernel::NotMapped) => fail(ENOMEM),
                Err(err) => Err(err),
            }
        }
        MADV_REMOVE | MADV_WIPEONFORK => fail(EINVAL),
        MADV_HWPOISON | MADV_SOFT_OFFLINE => fail(EPERM),
        // NORMAL, RANDOM, SEQUENTIAL and WILLNEED; DONTFORK and DOFORK;
        // MERGEABLE and UNMERGEABLE; HUGEPAGE and NOHUGEPAGE; DONTDUMP and
        // DODUMP; KEEPONFORK; COLD and PAGEOUT; POPULATE_READ and
        // POPULATE_WRITE; COLLAPSE.
        0..=3 | 10..=17 | 19..=23 | 25 => Ok(0),
        _ => fail(EINVAL),
    }
}

/// `brk(addr)`: moves the break to `addr` and returns it, or returns the
/// break unmoved if it may not go there: below where it started, or over
/// memory already mapped, or where no memory is left. The memory between
/// is the process's read-write anonymous memory.
pub fn brk(me: &ThreadRef, user: &UserRef, address: u64) -> u64 {
    // Held throughout, so that two threads move it one at a time; it is
    // taken before the address space's locks.
    let mut brk = user.process.linux.brk.lock();
    if address < brk.start || address > USER_TOP {
        return brk.end;
    }
    let (old_top, new_top) =
        (brk.end.next_multiple_of(PAGE_SIZE), address.next_multiple_of(PAGE_SIZE));
    let moved = match new_top.cmp(&old_top) {
        Ordering::Greater => {
            let args = abi::MapAnonymousArgs {
                address: old_top,
                size: new_top - old_top,
                rights: Rights::READ.union(Rights::WRITE),
                flags: MapFlags::FIXED_NO_REPLACE,
            };
            results(me, user, SLOT_SPACE, abi::MAP_ANONYMOUS, args.encode()).is_ok()
        }
        Ordering::Less => {
            let args = abi::DiscardArgs { address: new_top, size: old_top - new_top };
            results(me, user, SLOT_SPACE, abi::DISCARD, args.encode()).is_ok()
        }
        Ordering::Equal => true,
    };
    if moved {
        brk.end = address;
    }
    brk.end
}

/// `mremap`'s flags.
const MREMAP_MAYMOVE: u64 = 1;
const MREMAP_FIXED: u64 = 2;
const MREMAP_DONTUNMAP: u64 = 4;

/// `mremap(old_address, old_size, new_size, flags, new_address)` (ADR
/// 0629): the address space's `remap`, after Linux's checks in Linux's
/// order. A mapping shrinks in place, grows in place where the pages after
/// it are free, and otherwise moves with MREMAP_MAYMOVE, taking its pages
/// with it, as C libraries' `realloc` of a large block and Node.js's heap
/// do. Errors as Linux's `mm/mremap.c`: EFAULT where no mapping holds the
/// old range, ENOMEM where it cannot grow or move.
pub fn mremap(me: &ThreadRef, user: &UserRef, a: &Args) -> Result<u64> {
    let [address, old_size, new_size, flags, new_address, _] = *a;
    let moves = MREMAP_FIXED | MREMAP_DONTUNMAP;
    if flags & !(MREMAP_MAYMOVE | moves) != 0
        || flags & moves != 0 && flags & MREMAP_MAYMOVE == 0
        || flags & MREMAP_DONTUNMAP != 0 && old_size != new_size
        || !aligned(address)
    {
        return fail(EINVAL);
    }
    // Linux's PAGE_ALIGN, which wraps a size near the top to 0.
    let page_align = |size: u64| size.wrapping_add(PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let (old_size, new_size) = (page_align(old_size), page_align(new_size));
    if new_size == 0 {
        return fail(EINVAL);
    }
    if old_size > USER_TOP || new_size > USER_TOP {
        return fail(if flags & moves != 0 { EINVAL } else { ENOMEM });
    }
    let mut how = abi::RemapFlags::NONE;
    for (flag, native) in [
        (MREMAP_MAYMOVE, abi::RemapFlags::MAY_MOVE),
        (MREMAP_FIXED, abi::RemapFlags::FIXED),
        (MREMAP_DONTUNMAP, abi::RemapFlags::KEEP_OLD),
    ] {
        if flags & flag != 0 {
            how = how.union(native);
        }
    }
    // Linux's new address is a hint with MREMAP_DONTUNMAP alone, and
    // passed over without either.
    let to = if flags & moves != 0 { new_address } else { 0 };
    let args = abi::RemapArgs { address, size: old_size, new_size, flags: how, to };
    match results(me, user, SLOT_SPACE, abi::REMAP, args.encode()) {
        Ok(words) => Ok(abi::RemapResults::decode(&words)?.address),
        Err(Error::Native(status)) if is(status, Kernel::NotMapped) => fail(EFAULT),
        // A frame's, ring's or card's pages, as Linux's VM_PFNMAP: EFAULT to
        // grow, EINVAL to keep the old place.
        Err(Error::Native(status)) if is(status, Kernel::WrongKind) => {
            fail(if flags & MREMAP_DONTUNMAP != 0 { EINVAL } else { EFAULT })
        }
        Err(Error::Native(status))
            if is(status, Kernel::NoMemory) || is(status, Kernel::AlreadyMapped) =>
        {
            fail(ENOMEM)
        }
        Err(err) => Err(err),
    }
}

/// `memfd_create` (ADR 0614): the native `memory_file` (radix-process(2))
/// in the lowest free descriptor. As Linux's, in this order: EINVAL for a
/// flag it does not know (`MFD_HUGETLB` among them) or for `MFD_EXEC` with
/// `MFD_NOEXEC_SEAL`; EFAULT for a name that cannot be read; EINVAL for a
/// name of more than 249 bytes; EMFILE for no free descriptor. The native
/// method reads the name again from the program's memory.
pub fn memfd_create(me: &ThreadRef, user: &UserRef, address: u64, how: u64) -> Result<u64> {
    let flags = memfd::flags(how as u32).map_err(Error::Errno)?;
    let len = paths::read_string(address, memfd::MFD_NAME_MAX)
        .map_err(|err| match err {
            Error::Errno(ENAMETOOLONG) => Error::Errno(EINVAL),
            err => err,
        })?
        .len();
    let slot = files::into_free(user, 0, |slot| {
        let name = UserBuffer { address, len: len as u64 };
        let args = MemoryFileArgs { name, to: slot, flags };
        method(me, user, SLOT_PROCESS, process::MEMORY_FILE, args.encode()).map(|_| slot)
    })?;
    Ok(files::descriptor(slot))
}
