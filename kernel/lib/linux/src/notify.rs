// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's inotify: the `IN_` bits of an event mask and of a watch's
//! flags, and the layout of the `struct inotify_event` records a read of
//! an inotify descriptor gives.
//!
//! The kernel's watch files (`radix-process(2)`'s `watch_file`) take and
//! give Linux's bits and records as they are, so Linux's `inotify_init1`,
//! `inotify_add_watch` and `inotify_rm_watch` need no translation, and
//! Dukovo's C library reads the same records. What a watch file's reads
//! give is built by [`write_event`], and the rules for the mask of
//! `inotify_add_watch` are [`check_mask`]'s.

use crate::errno::EINVAL;

/// A file was read.
pub const IN_ACCESS: u32 = 0x1;
/// A file was written to, or cut.
pub const IN_MODIFY: u32 = 0x2;
/// A file's metadata changed: its access, its times, its extended
/// attributes or its number of links.
pub const IN_ATTRIB: u32 = 0x4;
/// A file opened for writing was closed.
pub const IN_CLOSE_WRITE: u32 = 0x8;
/// A file not opened for writing was closed.
pub const IN_CLOSE_NOWRITE: u32 = 0x10;
/// A file was opened.
pub const IN_OPEN: u32 = 0x20;
/// A file was moved out of the watched directory.
pub const IN_MOVED_FROM: u32 = 0x40;
/// A file was moved into the watched directory.
pub const IN_MOVED_TO: u32 = 0x80;
/// A file was made in the watched directory.
pub const IN_CREATE: u32 = 0x100;
/// A file was removed from the watched directory.
pub const IN_DELETE: u32 = 0x200;
/// The watched file or directory was removed.
pub const IN_DELETE_SELF: u32 = 0x400;
/// The watched file or directory was moved.
pub const IN_MOVE_SELF: u32 = 0x800;
/// The filesystem of the watched file was unmounted.
pub const IN_UNMOUNT: u32 = 0x2000;
/// The queue of events overflowed.
pub const IN_Q_OVERFLOW: u32 = 0x4000;
/// The watch is gone: removed, or its file was.
pub const IN_IGNORED: u32 = 0x8000;
/// Both closes.
pub const IN_CLOSE: u32 = IN_CLOSE_WRITE | IN_CLOSE_NOWRITE;
/// Both moves.
pub const IN_MOVE: u32 = IN_MOVED_FROM | IN_MOVED_TO;
/// Every event a watch may ask for.
pub const IN_ALL_EVENTS: u32 = IN_ACCESS
    | IN_MODIFY
    | IN_ATTRIB
    | IN_CLOSE
    | IN_OPEN
    | IN_MOVE
    | IN_CREATE
    | IN_DELETE
    | IN_DELETE_SELF
    | IN_MOVE_SELF;

/// A watch of a path that is not a directory fails with ENOTDIR.
pub const IN_ONLYDIR: u32 = 0x0100_0000;
/// A watch of a symbolic link is of the link, not what it leads to.
pub const IN_DONT_FOLLOW: u32 = 0x0200_0000;
/// A file unlinked from the watched directory sends it no events.
pub const IN_EXCL_UNLINK: u32 = 0x0400_0000;
/// A watch of a file that has one fails with EEXIST.
pub const IN_MASK_CREATE: u32 = 0x1000_0000;
/// A watch of a file that has one adds to its events.
pub const IN_MASK_ADD: u32 = 0x2000_0000;
/// The event is of a directory.
pub const IN_ISDIR: u32 = 0x4000_0000;
/// The watch goes after its first event.
pub const IN_ONESHOT: u32 = 0x8000_0000;

/// Every bit of an event mask and of the flags of a watch.
pub const IN_ALL_BITS: u32 = IN_ALL_EVENTS
    | IN_UNMOUNT
    | IN_Q_OVERFLOW
    | IN_IGNORED
    | IN_ONLYDIR
    | IN_DONT_FOLLOW
    | IN_EXCL_UNLINK
    | IN_MASK_CREATE
    | IN_MASK_ADD
    | IN_ISDIR
    | IN_ONESHOT;

/// The size of the fixed part of a `struct inotify_event`: the watch,
/// the mask, the cookie and the length of the name.
pub const EVENT_SIZE: usize = 16;

/// Checks the `mask` of `inotify_add_watch`, as Linux's does before it
/// looks at the path: EINVAL if it holds none of the bits there are, or
/// both `IN_MASK_ADD` and `IN_MASK_CREATE`.
///
/// # Errors
///
/// EINVAL, for those masks.
pub fn check_mask(mask: u32) -> Result<(), i32> {
    if mask & IN_ALL_BITS == 0 || (mask & IN_MASK_ADD != 0 && mask & IN_MASK_CREATE != 0) {
        return Err(EINVAL);
    }
    Ok(())
}

/// The length of the name field of an event whose name is `name_len`
/// bytes: the name and its NUL, padded with NULs to a multiple of
/// [`EVENT_SIZE`], as Linux does; 0 for an event with no name.
pub fn name_field_len(name_len: usize) -> usize {
    if name_len == 0 { 0 } else { (name_len + 1).next_multiple_of(EVENT_SIZE) }
}

/// The size of the record of an event whose name is `name_len` bytes.
pub fn record_len(name_len: usize) -> usize {
    EVENT_SIZE + name_field_len(name_len)
}

/// Writes the `struct inotify_event` for watch `wd` and `mask`, with
/// `cookie` and `name` (empty for none), to the start of `out`, which must
/// hold [`record_len`] bytes of it: the record's size.
///
/// # Panics
///
/// If `out` is shorter than the record.
pub fn write_event(wd: i32, mask: u32, cookie: u32, name: &[u8], out: &mut [u8]) -> usize {
    let field = name_field_len(name.len());
    let size = EVENT_SIZE + field;
    let record = &mut out[..size];
    record[0..4].copy_from_slice(&wd.to_le_bytes());
    record[4..8].copy_from_slice(&mask.to_le_bytes());
    record[8..12].copy_from_slice(&cookie.to_le_bytes());
    // The field is a few hundred bytes at most: a name has at most 255.
    record[12..16].copy_from_slice(&(field as u32).to_le_bytes());
    record[EVENT_SIZE..EVENT_SIZE + name.len()].copy_from_slice(name);
    record[EVENT_SIZE + name.len()..].fill(0);
    size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bits_are_linuxs() {
        assert_eq!(IN_ACCESS, 1);
        assert_eq!(IN_CLOSE, 0x18);
        assert_eq!(IN_MOVE, 0xc0);
        assert_eq!(IN_ALL_EVENTS, 0xfff);
        assert_eq!(IN_IGNORED, 0x8000);
        assert_eq!(IN_ONESHOT, 1 << 31);
        assert_eq!(IN_ALL_BITS, 0xf700_efff);
    }

    #[test]
    fn a_mask_needs_a_bit_and_never_both_add_and_create() {
        assert_eq!(check_mask(0), Err(EINVAL));
        assert_eq!(check_mask(0x1000), Err(EINVAL));
        assert_eq!(check_mask(IN_MASK_ADD | IN_MASK_CREATE | IN_MODIFY), Err(EINVAL));
        assert_eq!(check_mask(IN_MODIFY), Ok(()));
        assert_eq!(check_mask(IN_ONESHOT), Ok(()));
        assert_eq!(check_mask(IN_MASK_CREATE | IN_ALL_EVENTS), Ok(()));
    }

    #[test]
    fn a_name_is_padded_with_its_nul_to_sixteen_bytes() {
        assert_eq!(name_field_len(0), 0);
        assert_eq!(name_field_len(1), 16);
        assert_eq!(name_field_len(15), 16);
        assert_eq!(name_field_len(16), 32);
        assert_eq!(name_field_len(255), 256);
        assert_eq!(record_len(0), 16);
        assert_eq!(record_len(5), 32);
        assert_eq!(record_len(255), 272);
    }

    #[test]
    fn an_event_with_a_name_is_written_whole() {
        let mut out = [0xffu8; 64];
        let size = write_event(3, IN_CREATE | IN_ISDIR, 0, b"sub", &mut out);
        assert_eq!(size, 32);
        assert_eq!(out[0..4], 3i32.to_le_bytes());
        assert_eq!(out[4..8], (IN_CREATE | IN_ISDIR).to_le_bytes());
        assert_eq!(out[8..12], [0; 4]);
        assert_eq!(out[12..16], 16u32.to_le_bytes());
        assert_eq!(&out[16..19], b"sub");
        assert!(out[19..32].iter().all(|&b| b == 0));
        // Nothing past the record is touched.
        assert!(out[32..].iter().all(|&b| b == 0xff));
    }

    #[test]
    fn an_event_with_no_name_is_sixteen_bytes() {
        let mut out = [0xffu8; 32];
        assert_eq!(write_event(-1, IN_Q_OVERFLOW, 7, b"", &mut out), EVENT_SIZE);
        assert_eq!(out[0..4], (-1i32).to_le_bytes());
        assert_eq!(out[8..12], 7u32.to_le_bytes());
        assert_eq!(out[12..16], [0; 4]);
        assert!(out[16..].iter().all(|&b| b == 0xff));
    }

    #[test]
    fn the_longest_name_fits_in_its_record() {
        let name = [b'a'; 255];
        let mut out = [0u8; 272];
        assert_eq!(write_event(1, IN_DELETE, 0, &name, &mut out), 272);
        assert_eq!(out[12..16], 256u32.to_le_bytes());
        assert_eq!(out[16 + 255], 0);
    }
}
