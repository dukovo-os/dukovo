// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What Linux's calls on a descriptor have that the native files take in
//! another form: the owner `fcntl`'s `F_SETOWN` and `F_SETOWN_EX` name,
//! the flags of `preadv2` and `pwritev2`, and the record `FICLONERANGE`
//! reads.
//!
//! The native `owner` takes a kind and a number where Linux has a signed
//! process number, whose sign says it is a process group's, and a
//! `struct f_owner_ex`; this is the same translation Dukovo's C library
//! makes (`ports/mlibc/sysdeps/radix/generic/files.cpp`).

use radix_abi::file::OwnerKind;

use crate::errno::{EINVAL, EOPNOTSUPP, ESRCH};
use crate::sockets::get;

/// `fcntl`: make the process, or with a negative number the process
/// group, the owner of the description.
pub const F_SETOWN: u32 = 8;
/// `fcntl`: read the owner of the description, a process group's number
/// negated.
pub const F_GETOWN: u32 = 9;
/// `fcntl`: make the thread, process or process group a `struct
/// f_owner_ex` names the owner of the description.
pub const F_SETOWN_EX: u32 = 15;
/// `fcntl`: read the owner of the description as a `struct f_owner_ex`.
pub const F_GETOWN_EX: u32 = 16;

/// The size of `struct f_owner_ex`.
pub const F_OWNER_EX_SIZE: usize = 8;

/// `struct f_owner_ex`'s `type` for a thread.
pub const F_OWNER_TID: i32 = 0;
/// `struct f_owner_ex`'s `type` for a process.
pub const F_OWNER_PID: i32 = 1;
/// `struct f_owner_ex`'s `type` for a process group.
pub const F_OWNER_PGRP: i32 = 2;

/// `struct f_owner_ex`: whom `F_SETOWN_EX` names and `F_GETOWN_EX` gives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerEx {
    /// `type`: `F_OWNER_TID`, `F_OWNER_PID` or `F_OWNER_PGRP`.
    pub kind: i32,
    /// `pid`: the number of a thread, process or process group.
    pub pid: i32,
}

impl OwnerEx {
    /// Reads the record at `bytes`.
    pub fn read(bytes: &[u8; F_OWNER_EX_SIZE]) -> OwnerEx {
        OwnerEx { kind: i32::from_le_bytes(get(bytes, 0)), pid: i32::from_le_bytes(get(bytes, 4)) }
    }

    /// The record as Linux's `struct f_owner_ex` holds it.
    pub fn to_bytes(self) -> [u8; F_OWNER_EX_SIZE] {
        let mut out = [0u8; F_OWNER_EX_SIZE];
        out[..4].copy_from_slice(&self.kind.to_le_bytes());
        out[4..].copy_from_slice(&self.pid.to_le_bytes());
        out
    }
}

/// The owner `F_SETOWN`'s argument `pid` names, as the native `owner`
/// takes it: a process, or with a negative number a process group, and 0
/// for none (a process 0, which the native `owner` reads as none).
///
/// # Errors
///
/// EINVAL for `i32::MIN`, which has no group number to negate it to.
pub fn owner_of_pid(pid: i32) -> Result<(OwnerKind, u64), i32> {
    if pid == i32::MIN {
        return Err(EINVAL);
    }
    if pid < 0 {
        return Ok((OwnerKind::Group, u64::from(pid.unsigned_abs())));
    }
    Ok((OwnerKind::Process, u64::from(pid.unsigned_abs())))
}

/// The owner `owner` names, as the native `owner` takes it.
///
/// # Errors
///
/// EINVAL for a `type` that is not `F_OWNER_TID`, `F_OWNER_PID` or
/// `F_OWNER_PGRP`, and ESRCH for a negative number, which names no thread,
/// process or process group.
pub fn owner_of_ex(owner: OwnerEx) -> Result<(OwnerKind, u64), i32> {
    let kind = match owner.kind {
        F_OWNER_TID => OwnerKind::Thread,
        F_OWNER_PID => OwnerKind::Process,
        F_OWNER_PGRP => OwnerKind::Group,
        _ => return Err(EINVAL),
    };
    if owner.pid < 0 {
        return Err(ESRCH);
    }
    Ok((kind, u64::from(owner.pid.unsigned_abs())))
}

/// What `F_GETOWN` gives for the owner of `kind` and number `id`: the
/// number, negated for a process group.
pub fn owner_number(kind: OwnerKind, id: u64) -> i32 {
    // Numbers are 32 bits wide here (docs/deferred.md).
    let number = id as i32;
    if kind == OwnerKind::Group { number.wrapping_neg() } else { number }
}

/// The `type` of `struct f_owner_ex` for the owner of `kind`.
pub fn owner_type(kind: OwnerKind) -> i32 {
    match kind {
        OwnerKind::Thread => F_OWNER_TID,
        OwnerKind::Process => F_OWNER_PID,
        OwnerKind::Group => F_OWNER_PGRP,
    }
}

/// `preadv2` and `pwritev2`: high-priority, which a file that does not
/// poll for completion ignores, as Linux does.
pub const RWF_HIPRI: u32 = 0x1;
/// `pwritev2`: the write is followed by what `fdatasync` does.
pub const RWF_DSYNC: u32 = 0x2;
/// `pwritev2`: the write is followed by what `fsync` does.
pub const RWF_SYNC: u32 = 0x4;
/// `preadv2` and `pwritev2`: do not wait for data or room.
pub const RWF_NOWAIT: u32 = 0x8;
/// `pwritev2`: write at the end of the file, wherever the offset is.
pub const RWF_APPEND: u32 = 0x10;

/// What the flags of `preadv2` and `pwritev2` ask for that the native
/// read and write do not do by themselves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rwf {
    /// The write is followed by a sync.
    pub sync: bool,
    /// The write goes to the end of the file.
    pub append: bool,
}

/// The work `flags` of `preadv2` (`writing` false) or `pwritev2` ask for.
/// A read has none to do: `RWF_DSYNC`, `RWF_SYNC` and `RWF_APPEND` are for
/// writes, and Linux passes them to a read without effect.
///
/// # Errors
///
/// EOPNOTSUPP for `RWF_NOWAIT`, which no native file gives, and for any
/// flag Linux does not know or Dukovo does not do.
pub fn rw_flags(flags: u32, writing: bool) -> Result<Rwf, i32> {
    if flags & !(RWF_HIPRI | RWF_DSYNC | RWF_SYNC | RWF_APPEND) != 0 {
        return Err(EOPNOTSUPP);
    }
    if !writing {
        return Ok(Rwf::default());
    }
    Ok(Rwf { sync: flags & (RWF_DSYNC | RWF_SYNC) != 0, append: flags & RWF_APPEND != 0 })
}

/// The size of `struct file_clone_range`.
pub const FILE_CLONE_RANGE_SIZE: usize = 32;

/// `struct file_clone_range`, which `FICLONERANGE` reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CloneRange {
    /// `src_fd`: the descriptor of the file whose data is shared.
    pub src_fd: i64,
    /// `src_offset`: where in that file the data starts.
    pub src_offset: u64,
    /// `src_length`: how many bytes, 0 for to the end of that file.
    pub src_length: u64,
    /// `dest_offset`: where in the file the data goes.
    pub dest_offset: u64,
}

impl CloneRange {
    /// Reads the record at `bytes`.
    pub fn read(bytes: &[u8; FILE_CLONE_RANGE_SIZE]) -> CloneRange {
        CloneRange {
            src_fd: i64::from_le_bytes(get(bytes, 0)),
            src_offset: u64::from_le_bytes(get(bytes, 8)),
            src_length: u64::from_le_bytes(get(bytes, 16)),
            dest_offset: u64::from_le_bytes(get(bytes, 24)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f_owner_ex_has_linuxs_layout() {
        let owner = OwnerEx { kind: F_OWNER_PGRP, pid: 0x0102_0304 };
        let bytes = owner.to_bytes();
        assert_eq!(bytes, [2, 0, 0, 0, 4, 3, 2, 1]);
        assert_eq!(OwnerEx::read(&bytes), owner);
        assert_eq!(F_OWNER_EX_SIZE, 8);
    }

    #[test]
    fn a_negative_number_names_a_group() {
        assert_eq!(owner_of_pid(7), Ok((OwnerKind::Process, 7)));
        assert_eq!(owner_of_pid(-7), Ok((OwnerKind::Group, 7)));
        assert_eq!(owner_of_pid(0), Ok((OwnerKind::Process, 0)));
        assert_eq!(owner_of_pid(i32::MIN), Err(EINVAL));
        assert_eq!(owner_number(OwnerKind::Group, 7), -7);
        assert_eq!(owner_number(OwnerKind::Process, 7), 7);
        assert_eq!(owner_number(OwnerKind::Thread, 9), 9);
    }

    #[test]
    fn an_owner_of_ex_is_a_kind_and_a_number() {
        let named = |kind, pid| owner_of_ex(OwnerEx { kind, pid });
        assert_eq!(named(F_OWNER_TID, 5), Ok((OwnerKind::Thread, 5)));
        assert_eq!(named(F_OWNER_PID, 5), Ok((OwnerKind::Process, 5)));
        assert_eq!(named(F_OWNER_PGRP, 5), Ok((OwnerKind::Group, 5)));
        assert_eq!(named(3, 5), Err(EINVAL));
        assert_eq!(named(-1, 5), Err(EINVAL));
        assert_eq!(named(F_OWNER_PID, -1), Err(ESRCH));
        for kind in [OwnerKind::Thread, OwnerKind::Process, OwnerKind::Group] {
            assert_eq!(owner_of_ex(OwnerEx { kind: owner_type(kind), pid: 4 }), Ok((kind, 4)));
        }
    }

    #[test]
    fn reads_have_no_flag_work_to_do() {
        let all = RWF_HIPRI | RWF_DSYNC | RWF_SYNC | RWF_APPEND;
        assert_eq!(rw_flags(0, false), Ok(Rwf::default()));
        assert_eq!(rw_flags(all, false), Ok(Rwf::default()));
        assert_eq!(rw_flags(RWF_NOWAIT, false), Err(EOPNOTSUPP));
    }

    #[test]
    fn writes_sync_and_append_as_asked() {
        assert_eq!(rw_flags(0, true), Ok(Rwf::default()));
        assert_eq!(rw_flags(RWF_HIPRI, true), Ok(Rwf::default()));
        assert_eq!(rw_flags(RWF_DSYNC, true), Ok(Rwf { sync: true, append: false }));
        assert_eq!(rw_flags(RWF_SYNC, true), Ok(Rwf { sync: true, append: false }));
        assert_eq!(rw_flags(RWF_APPEND, true), Ok(Rwf { sync: false, append: true }));
        assert_eq!(rw_flags(RWF_NOWAIT, true), Err(EOPNOTSUPP));
        // The flags Linux has since: RWF_NOAPPEND, RWF_ATOMIC, RWF_DONTCACHE.
        for flag in [0x20, 0x40, 0x80, 0x100] {
            assert_eq!(rw_flags(flag, true), Err(EOPNOTSUPP));
        }
    }

    #[test]
    fn file_clone_range_has_linuxs_layout() {
        let mut bytes = [0u8; FILE_CLONE_RANGE_SIZE];
        bytes[..8].copy_from_slice(&(-1i64).to_le_bytes());
        bytes[8..16].copy_from_slice(&4096u64.to_le_bytes());
        bytes[16..24].copy_from_slice(&8192u64.to_le_bytes());
        bytes[24..].copy_from_slice(&12288u64.to_le_bytes());
        let range = CloneRange::read(&bytes);
        assert_eq!(
            range,
            CloneRange { src_fd: -1, src_offset: 4096, src_length: 8192, dest_offset: 12288 }
        );
        assert_eq!(FILE_CLONE_RANGE_SIZE, 32);
    }
}
