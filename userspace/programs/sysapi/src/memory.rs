// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Memory mapped from the kernel, as a benchmark maps it: fresh anonymous
//! pages, whose protection changes while nothing refers to them, and a
//! copy between two buffers with the instruction the kernel's own memory
//! benchmark copies with. These are inlined into a benchmark's loop, so
//! that each costs what its system call or instruction costs, as in C.
//!
//! And memory a program shares with the kernel by a file: a
//! [`MemoryFile`], which has no name, and the one [`SharedMapping`] of
//! its pages, as a picture's memory is shared with the display layer
//! (radix-display(2)).

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};

/// What the pages of a [`Mapping`] allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Nothing: a touch faults.
    None,
    /// Reads.
    Read,
    /// Reads and writes.
    ReadWrite,
}

impl Access {
    fn protection(self) -> libc::c_int {
        match self {
            Access::None => libc::PROT_NONE,
            Access::Read => libc::PROT_READ,
            Access::ReadWrite => libc::PROT_READ | libc::PROT_WRITE,
        }
    }
}

/// Fresh anonymous memory of the process's own (mmap(2) with
/// `MAP_PRIVATE | MAP_ANONYMOUS`), unmapped when it is dropped.
///
/// Its bytes are lent only while its pages allow what the loan does, and
/// [`Mapping::protect`] takes it mutably, so no loan outlives a change.
pub struct Mapping {
    at: NonNull<u8>,
    len: usize,
    access: Access,
}

impl Mapping {
    /// `len` bytes of zeroed memory that may be read and written, at an
    /// address the kernel picks, page-aligned.
    ///
    /// # Errors
    ///
    /// EINVAL for a length of 0, and ENOMEM when the process has no room.
    #[inline]
    pub fn anonymous(len: usize) -> io::Result<Mapping> {
        let access = Access::ReadWrite;
        // SAFETY: a private anonymous mapping at an address the kernel
        // picks touches no memory the process has.
        let at = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                access.protection(),
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if at == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let at =
            NonNull::new(at.cast()).ok_or_else(|| io::Error::from_raw_os_error(libc::ENOMEM))?;
        Ok(Mapping { at, len, access })
    }

    /// How many bytes it has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether it has no bytes, which a mapping never does.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Its bytes, while its pages may be read.
    #[must_use]
    pub fn bytes(&self) -> Option<&[u8]> {
        (self.access != Access::None).then(|| {
            // SAFETY: the mapping is `len` bytes from `at`, readable, and
            // stays so while `self` is borrowed: `protect` and the drop
            // take it mutably or by value.
            unsafe { std::slice::from_raw_parts(self.at.as_ptr(), self.len) }
        })
    }

    /// Its bytes to change, while its pages may be written.
    #[inline]
    pub fn bytes_mut(&mut self) -> Option<&mut [u8]> {
        (self.access == Access::ReadWrite).then(|| {
            // SAFETY: as in `bytes`, and writable; the mutable borrow of
            // `self` is the only loan.
            unsafe { std::slice::from_raw_parts_mut(self.at.as_ptr(), self.len) }
        })
    }

    /// Has its pages allow `access`, as mprotect(2) does.
    ///
    /// # Errors
    ///
    /// ENOMEM when the kernel has no room to split its records of the
    /// process's memory.
    #[inline]
    pub fn protect(&mut self, access: Access) -> io::Result<()> {
        // SAFETY: the range is this mapping's own, page-aligned, and no
        // loan of its bytes is alive while `self` is borrowed mutably.
        crate::checked(unsafe {
            libc::mprotect(self.at.as_ptr().cast(), self.len, access.protection())
        })?;
        self.access = access;
        Ok(())
    }
}

impl Drop for Mapping {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: the range is this mapping's own, unmapped once, and no
        // loan of its bytes outlives `self`.
        unsafe { libc::munmap(self.at.as_ptr().cast(), self.len) };
    }
}

/// Where memory files are made: the kernel's files of memory on Radix
/// (ADR 0490), and the build machine's temporary folder for the unit
/// tests.
#[cfg(target_os = "radix")]
fn memory_files() -> std::path::PathBuf {
    "/devices/shm".into()
}
#[cfg(not(target_os = "radix"))]
fn memory_files() -> std::path::PathBuf {
    std::env::temp_dir()
}

/// Counts the memory files the process has made, for their names.
static MADE: AtomicU32 = AtomicU32::new(0);

/// How many names [`MemoryFile::new`] tries: one that is taken was left
/// by a process of the same number that was killed between making its
/// file and taking the name away.
const NAMES_TRIED: u32 = 64;

/// A file of memory with no name: pages a program shares with the kernel
/// by the file's slot, as it shares a picture's with the display layer
/// (radix-display(2)). It is gone with its last descriptor and mapping.
///
/// The file is made here, under a name nothing else had, which is taken
/// away before the file is returned, and nothing gives its descriptor
/// out. So the program reaches its pages only through the one
/// [`SharedMapping`] it may make, and whatever else reaches them was
/// named the file's [`slot`](MemoryFile::slot) in a request of the
/// kernel's. That is why the mapping lends them as slices: see there.
#[derive(Debug)]
pub struct MemoryFile {
    file: File,
    /// Whether [`MemoryFile::map`] has mapped it.
    mapped: bool,
}

impl MemoryFile {
    /// A new, empty memory file.
    ///
    /// # Errors
    ///
    /// What open(2) and unlink(2) fail with: ENOSPC when the system has
    /// no room for another, EMFILE when the process has too many files
    /// open.
    pub fn new() -> io::Result<MemoryFile> {
        let folder = memory_files();
        let mut taken = None;
        for _ in 0..NAMES_TRIED {
            let number = MADE.fetch_add(1, Ordering::Relaxed);
            let path = folder.join(format!("memory-{}-{number}", std::process::id()));
            match File::options().read(true).write(true).create_new(true).open(&path) {
                Ok(file) => {
                    std::fs::remove_file(&path)?;
                    return Ok(MemoryFile { file, mapped: false });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => taken = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(taken.unwrap_or_else(|| io::Error::other("no name for a memory file")))
    }

    /// The slot it is in, which names it in a request of the kernel's
    /// that takes a memory file (MAKE_BUFFER, MAKE_PICTURE). Only a
    /// request that reads its pages is given it once it is mapped.
    #[must_use]
    pub fn slot(&self) -> u32 {
        crate::kernel::slot_of(&self.file)
    }

    /// How many bytes it has.
    ///
    /// # Errors
    ///
    /// What fstat(2) fails with.
    pub fn len(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    /// Whether it has no bytes yet.
    ///
    /// # Errors
    ///
    /// What fstat(2) fails with.
    pub fn is_empty(&self) -> io::Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Gives it `len` bytes: new ones are zero. The display layer sizes
    /// a picture's file itself (MAKE_BUFFER).
    ///
    /// # Errors
    ///
    /// `ErrorKind::InvalidInput` once it is mapped, so that the mapping
    /// keeps the bytes it has; what ftruncate(2) fails with: ENOSPC when
    /// the system has no room.
    pub fn set_len(&mut self, len: u64) -> io::Result<()> {
        if self.mapped {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a memory file keeps its length once it is mapped",
            ));
        }
        self.file.set_len(len)
    }

    /// Maps its first `len` bytes, to read and write. A file is mapped
    /// once: the mapping is the one way the program has to its pages.
    ///
    /// # Errors
    ///
    /// `ErrorKind::InvalidInput` when it was mapped before, when it has
    /// fewer than `len` bytes, so that no byte of the mapping is past
    /// the file's end, or when `len` is 0; what mmap(2) fails with:
    /// ENOMEM when the process has no room.
    pub fn map(&mut self, len: usize) -> io::Result<SharedMapping> {
        if self.mapped || len == 0 || self.len()? < len as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a memory file is mapped once, and for no more bytes than it has",
            ));
        }
        // SAFETY: a new mapping at an address the kernel picks touches no
        // memory the process has; the file is open, and `len` bytes of
        // it are there, as checked above.
        let at = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                self.file.as_raw_fd(),
                0,
            )
        };
        if at == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let at =
            NonNull::new(at.cast()).ok_or_else(|| io::Error::from_raw_os_error(libc::ENOMEM))?;
        self.mapped = true;
        Ok(SharedMapping { at, len })
    }
}

/// The pages of a [`MemoryFile`], mapped to read and write and unmapped
/// when it is dropped. What is written is in the file at once, for
/// whatever else shows or reads it.
///
/// Its bytes are lent as slices because nothing else writes them. The
/// program has no other way to them: the file gives out no descriptor,
/// is mapped once, and keeps its length from then on. And the rule for
/// the whole program is that a kernel request it names the file's slot
/// in only reads the pages, as the display layer reads a picture's.
#[derive(Debug)]
pub struct SharedMapping {
    at: NonNull<u8>,
    len: usize,
}

impl SharedMapping {
    /// How many bytes it has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether it has no bytes, which a mapping never does.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Its bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping is `len` bytes from `at`, readable, inside
        // the file as `map` checked and `set_len` keeps, and stays
        // mapped while `self` is borrowed; nothing writes the pages but
        // this process (the type's rule), and it only through
        // `bytes_mut`, which borrows `self` mutably.
        unsafe { std::slice::from_raw_parts(self.at.as_ptr(), self.len) }
    }

    /// Its bytes to change.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: as in `bytes`, and writable; the mutable borrow of
        // `self` is the only loan of the pages in this process, which
        // has no other mapping of the file and no descriptor of it.
        unsafe { std::slice::from_raw_parts_mut(self.at.as_ptr(), self.len) }
    }
}

impl Drop for SharedMapping {
    fn drop(&mut self) {
        // SAFETY: the range is this mapping's own, unmapped once, and no
        // loan of its bytes outlives `self`.
        unsafe { libc::munmap(self.at.as_ptr().cast(), self.len) };
    }
}

/// Copies `from` into `to` with `rep movsb`, as the kernel's memory
/// benchmark copies, so that the two measure the same instruction.
///
/// # Panics
///
/// When the two are not the same length.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn copy_bytes(to: &mut [u8], from: &[u8]) {
    assert_eq!(to.len(), from.len(), "a copy between buffers of different lengths");
    // SAFETY: rep movsb copies rcx bytes from rsi to rdi, upwards, as the
    // ABI leaves the direction flag clear; both slices are that long,
    // `to` may be written, and a mutable and a shared borrow never
    // overlap.
    unsafe {
        std::arch::asm!(
            "rep movsb",
            inout("rsi") from.as_ptr() => _,
            inout("rdi") to.as_mut_ptr() => _,
            inout("rcx") from.len() => _,
            options(nostack, preserves_flags),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mapping_is_zeroed_and_lends_only_what_its_pages_allow() {
        let mut mapping = Mapping::anonymous(8192).unwrap();
        assert_eq!(mapping.len(), 8192);
        assert!(mapping.bytes().unwrap().iter().all(|&b| b == 0));
        mapping.bytes_mut().unwrap()[4097] = 7;
        mapping.protect(Access::Read).unwrap();
        assert!(mapping.bytes_mut().is_none());
        assert_eq!(mapping.bytes().unwrap()[4097], 7);
        mapping.protect(Access::None).unwrap();
        assert!(mapping.bytes().is_none());
        mapping.protect(Access::ReadWrite).unwrap();
        assert_eq!(mapping.bytes_mut().unwrap()[4097], 7);
        assert!(Mapping::anonymous(0).is_err());
    }

    #[test]
    fn a_memory_file_is_mapped_once_within_its_length_and_has_no_name() {
        let mut file = MemoryFile::new().unwrap();
        assert!(file.is_empty().unwrap());
        assert!(file.map(4096).is_err(), "no mapping past the file's end");
        file.set_len(8192).unwrap();
        assert!(file.map(0).is_err());
        assert!(file.map(8193).is_err());
        let mut mapping = file.map(8192).unwrap();
        assert_eq!((mapping.len(), mapping.is_empty()), (8192, false));
        assert!(mapping.bytes().iter().all(|&byte| byte == 0));
        mapping.bytes_mut()[5000] = 9;
        assert_eq!(mapping.bytes()[5000], 9);
        // The pages are the file's: what reads the file reads them.
        let mut read = [0u8; 1];
        std::os::unix::fs::FileExt::read_exact_at(&file.file, &mut read, 5000).unwrap();
        assert_eq!(read, [9]);
        // The mapping is the one way to them, and they stay under it.
        assert!(file.map(4096).is_err(), "a second mapping");
        assert!(file.set_len(4096).is_err(), "a mapped file made shorter");
        assert_eq!(file.len().unwrap(), 8192);
        // The mapping outlives the file's descriptor.
        drop(file);
        assert_eq!(mapping.bytes()[5000], 9);
        // Each file is another, and none is left in the folder.
        let second = MemoryFile::new().unwrap();
        assert!(second.is_empty().unwrap());
        let ours = format!("memory-{}-", std::process::id());
        let mut left = std::fs::read_dir(memory_files()).unwrap().flatten();
        assert!(!left.any(|entry| entry.file_name().to_string_lossy().starts_with(&ours)));
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_copy_copies_every_byte() {
        let from: Vec<u8> = (0..=255).cycle().take(5000).collect();
        let mut to = vec![0u8; 5000];
        copy_bytes(&mut to, &from);
        assert_eq!(to, from);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    #[should_panic(expected = "different lengths")]
    fn a_copy_between_lengths_is_refused() {
        copy_bytes(&mut [0u8; 2], &[1u8; 3]);
    }
}
