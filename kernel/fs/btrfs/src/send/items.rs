// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The items of a subvolume's tree that a send reads, as the format lays
//! them out (btrfs.readthedocs.io, "On-disk format"): every number
//! little-endian, and every item checked for its size before it is read,
//! since a tree's items come from a disk.

use std::io;
use std::vec::Vec;

use super::stream::Time;

/// Why an item could not be read.
pub(crate) fn corrupt(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, std::format!("a damaged {what} item"))
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&data[at..at + 4]);
    u32::from_le_bytes(word)
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&data[at..at + 8]);
    u64::from_le_bytes(word)
}

fn time_at(data: &[u8], at: usize) -> Time {
    Time { secs: u64_at(data, at) as i64, nanos: u32_at(data, at + 8) }
}

/// The kinds of file, from a mode's `S_IFMT` bits.
pub mod kind {
    /// The bits of a mode that say its kind.
    pub const MASK: u32 = 0o170000;
    /// A socket's name.
    pub const SOCKET: u32 = 0o140000;
    /// A symbolic link.
    pub const LINK: u32 = 0o120000;
    /// A file of bytes.
    pub const FILE: u32 = 0o100000;
    /// A block device.
    pub const BLOCK: u32 = 0o060000;
    /// A folder.
    pub const DIRECTORY: u32 = 0o040000;
    /// A character device.
    pub const CHAR: u32 = 0o020000;
    /// A named pipe.
    pub const FIFO: u32 = 0o010000;
}

/// What an inode item says of a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inode {
    /// The transaction that made it: an inode number used again is
    /// another file.
    pub generation: u64,
    /// Its size in bytes.
    pub size: u64,
    /// How many names it has.
    pub links: u32,
    /// Its owner.
    pub uid: u32,
    /// Its group.
    pub gid: u32,
    /// Its kind and permission bits.
    pub mode: u32,
    /// The device number a device stands for.
    pub rdev: u64,
    /// When it was last read.
    pub atime: Time,
    /// When it last changed.
    pub ctime: Time,
    /// When its contents last changed.
    pub mtime: Time,
}

impl Inode {
    /// An inode item's size.
    pub const SIZE: usize = 160;

    /// The inode item `data`.
    pub fn parse(data: &[u8]) -> io::Result<Inode> {
        if data.len() < Inode::SIZE {
            return Err(corrupt("inode"));
        }
        Ok(Inode {
            generation: u64_at(data, 0),
            size: u64_at(data, 16),
            links: u32_at(data, 40),
            uid: u32_at(data, 44),
            gid: u32_at(data, 48),
            mode: u32_at(data, 52),
            rdev: u64_at(data, 56),
            atime: time_at(data, 112),
            ctime: time_at(data, 124),
            mtime: time_at(data, 136),
        })
    }

    /// Whether it is a folder.
    pub fn is_dir(&self) -> bool {
        self.mode & kind::MASK == kind::DIRECTORY
    }

    /// Whether it is a symbolic link.
    pub fn is_link(&self) -> bool {
        self.mode & kind::MASK == kind::LINK
    }
}

/// The names an inode reference item (`INODE_REF`, keyed by the folder)
/// or extended one (`INODE_EXTREF`, keyed by a hash) gives a file: the
/// folder each is in, and the name.
pub fn names(kind: u8, folder: u64, data: &[u8], out: &mut Vec<(u64, Vec<u8>)>) -> io::Result<()> {
    let mut rest = data;
    while !rest.is_empty() {
        let (parent, header) = match kind {
            crate::format::item::INODE_REF => (folder, 10),
            _ => {
                if rest.len() < 18 {
                    return Err(corrupt("inode reference"));
                }
                (u64_at(rest, 0), 18)
            }
        };
        if rest.len() < header {
            return Err(corrupt("inode reference"));
        }
        let len = usize::from(u16_at(rest, header - 2));
        let Some(name) = rest.get(header..header + len) else {
            return Err(corrupt("inode reference"));
        };
        out.push((parent, name.to_vec()));
        rest = &rest[header + len..];
    }
    Ok(())
}

/// The extended attributes an `XATTR_ITEM` holds, as names and values:
/// more than one when names share a hash.
pub fn xattrs(data: &[u8]) -> io::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    const HEADER: usize = 30;
    let mut out = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        if rest.len() < HEADER {
            return Err(corrupt("extended attribute"));
        }
        let data_len = usize::from(u16_at(rest, 25));
        let name_len = usize::from(u16_at(rest, 27));
        let end = HEADER + name_len + data_len;
        if rest.len() < end {
            return Err(corrupt("extended attribute"));
        }
        out.push((rest[HEADER..HEADER + name_len].to_vec(), rest[HEADER + name_len..end].to_vec()));
        rest = &rest[end..];
    }
    Ok(out)
}

/// The kinds of extent.
pub mod extent {
    /// Data in the item itself.
    pub const INLINE: u8 = 0;
    /// Data on the disk, or a hole.
    pub const REGULAR: u8 = 1;
    /// Room on the disk taken and not written: zeros.
    pub const PREALLOC: u8 = 2;
}

/// What an `EXTENT_DATA` item covers of its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    /// Its bytes of the file, from the item's key offset.
    pub len: u64,
    /// Whether it holds no data: a hole, or room taken and not written.
    pub empty: bool,
}

impl Extent {
    /// The extent item `data`.
    pub fn parse(data: &[u8]) -> io::Result<Extent> {
        if data.len() < 21 {
            return Err(corrupt("extent"));
        }
        match data[20] {
            // An inline extent's bytes are its uncompressed size.
            extent::INLINE => Ok(Extent { len: u64_at(data, 8), empty: false }),
            kind @ (extent::REGULAR | extent::PREALLOC) => {
                if data.len() < 53 {
                    return Err(corrupt("extent"));
                }
                let disk_bytenr = u64_at(data, 21);
                Ok(Extent {
                    len: u64_at(data, 45),
                    empty: kind == extent::PREALLOC || disk_bytenr == 0,
                })
            }
            _ => Err(corrupt("extent")),
        }
    }
}

/// The location a `DIR_ITEM` or `DIR_INDEX` names: its inode, or for a
/// subvolume's entry, its root item, and the name.
pub fn dir_entries(data: &[u8]) -> io::Result<Vec<(u64, u8, Vec<u8>)>> {
    const HEADER: usize = 30;
    let mut out = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        if rest.len() < HEADER {
            return Err(corrupt("directory"));
        }
        let data_len = usize::from(u16_at(rest, 25));
        let name_len = usize::from(u16_at(rest, 27));
        let end = HEADER + name_len + data_len;
        if rest.len() < end {
            return Err(corrupt("directory"));
        }
        out.push((u64_at(rest, 0), rest[8], rest[HEADER..HEADER + name_len].to_vec()));
        rest = &rest[end..];
    }
    Ok(out)
}

/// The device number an inode item holds (the kernel's: the major number
/// above bit 20) as a stream carries it, the way programs see device
/// numbers (Linux's `new_encode_dev`).
pub fn stream_rdev(rdev: u64) -> u64 {
    let (major, minor) = (rdev >> 20 & 0xfff, rdev & 0xf_ffff);
    (minor & 0xff) | (major << 8) | ((minor & !0xff) << 12)
}

/// The major and minor numbers of a device number as a stream carries it.
pub fn device_parts(rdev: u64) -> (u32, u32) {
    let major = ((rdev >> 8) & 0xfff) | ((rdev >> 32) & !0xfff);
    let minor = (rdev & 0xff) | ((rdev >> 12) & !0xff);
    (major as u32, minor as u32)
}
