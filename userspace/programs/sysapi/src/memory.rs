// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Memory mapped from the kernel, as a benchmark maps it: fresh anonymous
//! pages, whose protection changes while nothing refers to them, and a
//! copy between two buffers with the instruction the kernel's own memory
//! benchmark copies with.

use std::io;
use std::ptr::NonNull;

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
