// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The flags of Linux's `memfd_create` and the seal commands of `fcntl`,
//! and what they ask of the native `memory_file` and `seal` (ADR 0614).
//!
//! Linux's `F_SEAL_` bits are the native [`Seals`] bits, so that a seal
//! argument passes through once it is checked against the ones Linux
//! knows. The values are those of Linux's `linux/memfd.h` and
//! `linux/fcntl.h`.

use radix_abi::file::Seals;
use radix_abi::process::MemoryFileFlags;

use crate::errno::EINVAL;

/// `memfd_create`: close the descriptor at `exec`.
pub const MFD_CLOEXEC: u32 = 0x1;
/// `memfd_create`: the file may be sealed.
pub const MFD_ALLOW_SEALING: u32 = 0x2;
/// `memfd_create`: a file of huge pages, which memory files are not.
pub const MFD_HUGETLB: u32 = 0x4;
/// `memfd_create`: the file may never be made executable, and is sealed so.
pub const MFD_NOEXEC_SEAL: u32 = 0x8;
/// `memfd_create`: the file may be made executable (what Linux does when
/// neither this nor [`MFD_NOEXEC_SEAL`] is given).
pub const MFD_EXEC: u32 = 0x10;

/// The longest name `memfd_create` takes, without its NUL: Linux's NAME_MAX
/// of 255 less the `memfd:` that its path starts with.
pub const MFD_NAME_MAX: usize = 249;

/// `fcntl`: add seals to a memory file.
pub const F_ADD_SEALS: u32 = 1033;
/// `fcntl`: read the seals of a memory file.
pub const F_GET_SEALS: u32 = 1034;

/// The native flags of the memory file `memfd_create` makes with
/// `linux_flags`.
///
/// # Errors
///
/// EINVAL for a flag Linux does not know, `MFD_HUGETLB` and the huge page
/// sizes beside it included (memory files are not of huge pages), and for
/// `MFD_EXEC` with `MFD_NOEXEC_SEAL`.
pub fn flags(linux_flags: u32) -> Result<MemoryFileFlags, i32> {
    let known = MFD_CLOEXEC | MFD_ALLOW_SEALING | MFD_NOEXEC_SEAL | MFD_EXEC;
    let both_exec = linux_flags & MFD_EXEC != 0 && linux_flags & MFD_NOEXEC_SEAL != 0;
    if linux_flags & !known != 0 || both_exec {
        return Err(EINVAL);
    }
    let mut native = MemoryFileFlags::NONE;
    for (bit, flag) in [
        (MFD_CLOEXEC, MemoryFileFlags::CLOSE_ON_EXEC),
        (MFD_ALLOW_SEALING, MemoryFileFlags::ALLOW_SEALING),
        (MFD_NOEXEC_SEAL, MemoryFileFlags::NO_EXEC),
    ] {
        if linux_flags & bit != 0 {
            native = native.union(flag);
        }
    }
    Ok(native)
}

/// The native seals `F_ADD_SEALS` asks for with the argument `argument`.
///
/// # Errors
///
/// EINVAL for a bit that is no seal of Linux's.
pub fn seals(argument: u32) -> Result<Seals, i32> {
    Seals::from_bits(argument).ok_or(EINVAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_numbers_are_linuxs() {
        assert_eq!(MFD_CLOEXEC, 1);
        assert_eq!(MFD_ALLOW_SEALING, 2);
        assert_eq!(MFD_HUGETLB, 4);
        assert_eq!(MFD_NOEXEC_SEAL, 8);
        assert_eq!(MFD_EXEC, 0x10);
        assert_eq!(F_ADD_SEALS, 1033);
        assert_eq!(F_GET_SEALS, 1034);
        assert_eq!(MFD_NAME_MAX + "memfd:".len(), 255);
    }

    #[test]
    fn the_seals_are_linuxs_bits() {
        assert_eq!(Seals::SEAL.bits(), 0x1);
        assert_eq!(Seals::SHRINK.bits(), 0x2);
        assert_eq!(Seals::GROW.bits(), 0x4);
        assert_eq!(Seals::WRITE.bits(), 0x8);
        assert_eq!(Seals::FUTURE_WRITE.bits(), 0x10);
        assert_eq!(Seals::EXEC.bits(), 0x20);
    }

    #[test]
    fn each_flag_asks_for_its_native_flag() {
        assert_eq!(flags(0), Ok(MemoryFileFlags::NONE));
        assert_eq!(flags(MFD_CLOEXEC), Ok(MemoryFileFlags::CLOSE_ON_EXEC));
        assert_eq!(flags(MFD_ALLOW_SEALING), Ok(MemoryFileFlags::ALLOW_SEALING));
        assert_eq!(flags(MFD_NOEXEC_SEAL), Ok(MemoryFileFlags::NO_EXEC));
        // A file that may run is the native default.
        assert_eq!(flags(MFD_EXEC), Ok(MemoryFileFlags::NONE));
        let all = MFD_CLOEXEC | MFD_ALLOW_SEALING | MFD_NOEXEC_SEAL;
        let native = MemoryFileFlags::CLOSE_ON_EXEC
            .union(MemoryFileFlags::ALLOW_SEALING)
            .union(MemoryFileFlags::NO_EXEC);
        assert_eq!(flags(all), Ok(native));
    }

    #[test]
    fn huge_pages_unknown_flags_and_both_exec_flags_are_refused() {
        assert_eq!(flags(MFD_HUGETLB), Err(EINVAL));
        assert_eq!(flags(MFD_HUGETLB | MFD_CLOEXEC), Err(EINVAL));
        assert_eq!(flags(0x20), Err(EINVAL));
        assert_eq!(flags(1 << 26), Err(EINVAL));
        assert_eq!(flags(MFD_EXEC | MFD_NOEXEC_SEAL), Err(EINVAL));
        assert_eq!(flags(u32::MAX), Err(EINVAL));
    }

    #[test]
    fn a_seal_argument_is_checked_against_linuxs_seals() {
        assert_eq!(seals(0), Ok(Seals::NONE));
        assert_eq!(seals(0x3f), Ok(Seals::ALL));
        assert_eq!(seals(0x8 | 0x2), Ok(Seals::WRITE.union(Seals::SHRINK)));
        assert_eq!(seals(0x40), Err(EINVAL));
        assert_eq!(seals(u32::MAX), Err(EINVAL));
    }
}
