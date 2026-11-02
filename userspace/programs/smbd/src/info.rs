// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a file is, in Windows' words (MS-FSCC 2.4 and 2.5): its times,
//! sizes and attributes, as QUERY_INFO and QUERY_DIRECTORY give them, and
//! what the filesystem is.
//!
//! A file's attributes come from its mode, as Samba gives them with its
//! defaults: a folder is DIRECTORY, a file without its owner's write bit
//! READONLY, a name that starts with a dot HIDDEN, and a file with none
//! of these NORMAL. Its creation time is the earlier of its change and
//! modification times, as no birth time is kept.

use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;

use crate::wire::{self, Out, Status, status};

/// The attributes (MS-FSCC 2.6).
pub mod attribute {
    pub const READONLY: u32 = 0x0000_0001;
    pub const HIDDEN: u32 = 0x0000_0002;
    pub const DIRECTORY: u32 = 0x0000_0010;
    pub const NORMAL: u32 = 0x0000_0080;
}

/// A file, as Windows sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    /// When it was made, in Windows' time.
    pub creation: u64,
    /// When it was last read.
    pub access: u64,
    /// When it was last written.
    pub write: u64,
    /// When it last changed, its data or its metadata.
    pub change: u64,
    /// Its size.
    pub size: u64,
    /// The space it takes.
    pub allocation: u64,
    /// Its attributes.
    pub attributes: u32,
    /// Its names in the filesystem.
    pub links: u32,
    /// Its number in the filesystem.
    pub inode: u64,
    /// Its filesystem's.
    pub device: u64,
    /// Whether it is a folder.
    pub directory: bool,
}

impl Stat {
    /// The file `meta` describes, named `name`.
    pub fn of(meta: &Metadata, name: &str) -> Stat {
        let write = wire::filetime(meta.mtime(), meta.mtime_nsec());
        let change = wire::filetime(meta.ctime(), meta.ctime_nsec());
        let directory = meta.is_dir();
        let mut attributes = 0;
        if directory {
            attributes |= attribute::DIRECTORY;
        } else if meta.mode() & 0o200 == 0 {
            attributes |= attribute::READONLY;
        }
        if name.starts_with('.') && name != "." && name != ".." {
            attributes |= attribute::HIDDEN;
        }
        if attributes == 0 {
            attributes = attribute::NORMAL;
        }
        Stat {
            creation: write.min(change),
            access: wire::filetime(meta.atime(), meta.atime_nsec()),
            write,
            change,
            size: if directory { 0 } else { meta.size() },
            allocation: if directory { 0 } else { meta.blocks() * 512 },
            attributes,
            links: meta.nlink() as u32,
            inode: meta.ino(),
            device: meta.dev(),
            directory,
        }
    }

    /// The four times, as most classes start.
    fn times(&self, out: &mut Out) {
        out.u64(self.creation).u64(self.access).u64(self.write).u64(self.change);
    }
}

/// The file information classes smbd answers (MS-FSCC 2.4).
pub mod class {
    pub const DIRECTORY: u8 = 1;
    pub const FULL_DIRECTORY: u8 = 2;
    pub const BOTH_DIRECTORY: u8 = 3;
    pub const BASIC: u8 = 4;
    pub const STANDARD: u8 = 5;
    pub const INTERNAL: u8 = 6;
    pub const EA: u8 = 7;
    pub const ACCESS: u8 = 8;
    pub const NAME: u8 = 9;
    pub const RENAME: u8 = 10;
    pub const LINK: u8 = 11;
    pub const NAMES: u8 = 12;
    pub const DISPOSITION: u8 = 13;
    pub const POSITION: u8 = 14;
    pub const FULL_EA: u8 = 15;
    pub const MODE: u8 = 16;
    pub const ALIGNMENT: u8 = 17;
    pub const ALL: u8 = 18;
    pub const ALLOCATION: u8 = 19;
    pub const END_OF_FILE: u8 = 20;
    pub const ALTERNATE_NAME: u8 = 21;
    pub const STREAM: u8 = 22;
    pub const COMPRESSION: u8 = 28;
    pub const NETWORK_OPEN: u8 = 34;
    pub const ATTRIBUTE_TAG: u8 = 35;
    pub const ID_BOTH_DIRECTORY: u8 = 37;
    pub const ID_FULL_DIRECTORY: u8 = 38;
    pub const NORMALIZED_NAME: u8 = 48;
    pub const ID: u8 = 59;
    pub const DISPOSITION_EX: u8 = 64;
}

/// The entry of a directory listing of `class` for `stat`, named `name`,
/// without its NextEntryOffset, which the listing fills in.
pub fn entry(class: u8, stat: &Stat, name: &str) -> Result<Vec<u8>, Status> {
    let name = wire::to_utf16(name);
    let mut out = Out::new();
    out.u32(0).u32(0);
    if class == class::NAMES {
        out.u32(name.len() as u32).bytes(&name);
        return Ok(out.0);
    }
    stat.times(&mut out);
    out.u64(stat.size).u64(stat.allocation).u32(stat.attributes).u32(name.len() as u32);
    match class {
        class::DIRECTORY => {}
        class::FULL_DIRECTORY => {
            out.u32(0);
        }
        class::ID_FULL_DIRECTORY => {
            out.u32(0).u32(0).u64(stat.inode);
        }
        class::BOTH_DIRECTORY => {
            // No 8.3 name.
            out.u32(0).u8(0).u8(0).zeros(24);
        }
        class::ID_BOTH_DIRECTORY => {
            out.u32(0).u8(0).u8(0).zeros(24).u16(0).u64(stat.inode);
        }
        _ => return Err(status::INVALID_INFO_CLASS),
    }
    out.bytes(&name);
    Ok(out.0)
}

/// What QUERY_INFO's file classes need of an open beyond its stat.
pub struct Opened<'a> {
    /// Its path within the share, with a backslash first.
    pub name: &'a str,
    /// The access it was granted.
    pub access: u32,
    /// Its position, for FilePositionInformation.
    pub position: u64,
    /// Whether it is to be deleted when closed.
    pub delete_pending: bool,
}

/// A file information class of `stat` and `open` (MS-FSCC 2.4).
pub fn file(class: u8, stat: &Stat, open: &Opened) -> Result<Vec<u8>, Status> {
    let mut out = Out::new();
    let name = wire::to_utf16(open.name);
    match class {
        class::BASIC => basic(&mut out, stat),
        class::STANDARD => standard(&mut out, stat, open.delete_pending),
        class::INTERNAL => {
            out.u64(stat.inode);
        }
        class::EA => {
            out.u32(0);
        }
        class::ACCESS => {
            out.u32(open.access);
        }
        class::POSITION => {
            out.u64(open.position);
        }
        class::MODE | class::ALIGNMENT => {
            out.u32(0);
        }
        class::NAME | class::NORMALIZED_NAME => {
            out.u32(name.len() as u32).bytes(&name);
        }
        class::ALL => {
            basic(&mut out, stat);
            standard(&mut out, stat, open.delete_pending);
            out.u64(stat.inode).u32(0).u32(open.access).u64(open.position).u32(0).u32(0);
            out.u32(name.len() as u32).bytes(&name);
        }
        class::NETWORK_OPEN => {
            stat.times(&mut out);
            out.u64(stat.allocation).u64(stat.size).u32(stat.attributes).u32(0);
        }
        class::ATTRIBUTE_TAG => {
            out.u32(stat.attributes).u32(0);
        }
        class::STREAM => {
            // One stream, the data's; a folder has none.
            if !stat.directory {
                let data = wire::to_utf16("::$DATA");
                out.u32(0).u32(data.len() as u32).u64(stat.size).u64(stat.allocation).bytes(&data);
            }
        }
        class::COMPRESSION => {
            out.u64(stat.size).u16(0).u8(0).u8(0).u8(0).zeros(3);
        }
        class::ID => {
            out.u64(stat.device).u64(stat.inode).u64(0);
        }
        // No 8.3 names: as NTFS with them off answers.
        class::ALTERNATE_NAME => return Err(status::OBJECT_NAME_NOT_FOUND),
        _ => return Err(status::INVALID_INFO_CLASS),
    }
    Ok(out.0)
}

/// FileBasicInformation.
fn basic(out: &mut Out, stat: &Stat) {
    stat.times(out);
    out.u32(stat.attributes).u32(0);
}

/// FileStandardInformation.
fn standard(out: &mut Out, stat: &Stat, delete_pending: bool) {
    out.u64(stat.allocation)
        .u64(stat.size)
        .u32(stat.links)
        .u8(u8::from(delete_pending))
        .u8(u8::from(stat.directory))
        .u16(0);
}

/// Whether a class's answer may be cut short to the buffer, with
/// STATUS_BUFFER_OVERFLOW: those that end with a name or a list.
pub fn may_be_cut(class: u8) -> bool {
    matches!(class, class::ALL | class::NAME | class::NORMALIZED_NAME | class::STREAM)
}

/// The filesystem information classes (MS-FSCC 2.5).
pub mod fs_class {
    pub const VOLUME: u8 = 1;
    pub const SIZE: u8 = 3;
    pub const DEVICE: u8 = 4;
    pub const ATTRIBUTE: u8 = 5;
    pub const FULL_SIZE: u8 = 7;
    pub const OBJECT_ID: u8 = 8;
    pub const SECTOR_SIZE: u8 = 11;
}

/// What a filesystem is, for its classes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Volume {
    /// Its size, in blocks.
    pub blocks: u64,
    /// The blocks free for the person.
    pub available: u64,
    /// The blocks free at all.
    pub free: u64,
    /// The size of a block.
    pub block_size: u32,
    /// Its serial number.
    pub serial: u32,
    /// The longest name it keeps.
    pub name_max: u32,
}

/// A filesystem information class of `volume`, labelled `label`.
pub fn volume(class: u8, volume: &Volume, label: &str) -> Result<Vec<u8>, Status> {
    let mut out = Out::new();
    // Allocation units of the block size, in sectors of 512 bytes.
    let sectors = (volume.block_size / 512).max(1);
    match class {
        fs_class::VOLUME => {
            let label = wire::to_utf16(label);
            out.u64(0).u32(volume.serial).u32(label.len() as u32).u8(0).u8(0).bytes(&label);
        }
        fs_class::SIZE => {
            out.u64(volume.blocks).u64(volume.available).u32(sectors).u32(512);
        }
        fs_class::FULL_SIZE => {
            out.u64(volume.blocks).u64(volume.available).u64(volume.free).u32(sectors).u32(512);
        }
        fs_class::DEVICE => {
            // FILE_DEVICE_DISK; FILE_DEVICE_IS_MOUNTED.
            out.u32(0x0000_0007).u32(0x0000_0020);
        }
        fs_class::ATTRIBUTE => {
            // Names keep their case and are compared with it, in Unicode.
            // The name is the one Windows' programs know, as Samba says
            // it by default: some check for it before they write.
            let name = wire::to_utf16("NTFS");
            out.u32(0x0000_0007).u32(volume.name_max).u32(name.len() as u32).bytes(&name);
        }
        fs_class::SECTOR_SIZE => {
            out.u32(512).u32(512).u32(512).u32(512).u32(0x0000_0003).u32(0).u32(0);
        }
        fs_class::OBJECT_ID => return Err(status::NOT_SUPPORTED),
        _ => return Err(status::INVALID_INFO_CLASS),
    }
    Ok(out.0)
}

/// The parts of a security descriptor QUERY_INFO asks for.
pub mod security {
    pub const OWNER: u32 = 0x1;
    pub const GROUP: u32 = 0x2;
    pub const DACL: u32 = 0x4;
}

/// A SID's bytes: revision 1, `authority`, then `subs`.
fn sid(authority: u8, subs: &[u32]) -> Vec<u8> {
    let mut out = Out::new();
    out.u8(1).u8(subs.len() as u8).bytes(&[0, 0, 0, 0, 0, authority]);
    for &sub in subs {
        out.u32(sub);
    }
    out.0
}

/// The rights of a mode's three bits, `rwx`, as an access mask.
fn rights(bits: u32, owner: bool) -> u32 {
    // READ_CONTROL and SYNCHRONIZE, and reading attributes, always.
    let mut mask = 0x0012_0080;
    if bits & 4 != 0 {
        mask |= 0x0012_0089;
    }
    if bits & 2 != 0 {
        mask |= 0x0012_0116 | 0x0001_0040;
    }
    if bits & 1 != 0 {
        mask |= 0x0012_00a0;
    }
    if owner {
        // The owner changes its modes, as WRITE_DAC and WRITE_ATTRIBUTES.
        mask |= 0x0004_0100;
    }
    mask
}

/// A self-relative security descriptor of a file owned by `uid` and
/// `gid` with `mode` (MS-DTYP 2.4.6): Samba's SIDs for Unix's users and
/// groups (S-1-22-1-uid and S-1-22-2-gid), and a DACL of an allowing ACE
/// for the owner, the group and Everyone, from the mode's three triples.
/// `parts` says which of owner, group and DACL to give.
pub fn descriptor(uid: u32, gid: u32, mode: u32, parts: u32) -> Vec<u8> {
    let owner = sid(22, &[1, uid]);
    let group = sid(22, &[2, gid]);
    let everyone = sid(1, &[0]);
    let mut dacl = Out::new();
    let aces = [
        (rights(mode >> 6, true), &owner),
        (rights(mode >> 3, false), &group),
        (rights(mode, false), &everyone),
    ];
    dacl.u8(2).u8(0).u16(0).u16(aces.len() as u16).u16(0);
    for (mask, who) in aces {
        dacl.u8(0).u8(0).u16((8 + who.len()) as u16).u32(mask).bytes(who);
    }
    let size = dacl.len() as u16;
    dacl.set_u16(2, size);
    let mut out = Out::new();
    // SE_SELF_RELATIVE, with a DACL when one is asked for.
    let control = 0x8000 | if parts & security::DACL != 0 { 0x0004 } else { 0 };
    out.u8(1).u8(0).u16(control).u32(0).u32(0).u32(0).u32(0);
    let put = |out: &mut Out, field: usize, bytes: &[u8]| {
        let at = out.len() as u32;
        out.set_u32(field, at);
        out.bytes(bytes);
    };
    if parts & security::OWNER != 0 {
        put(&mut out, 4, &owner);
    }
    if parts & security::GROUP != 0 {
        put(&mut out, 8, &group);
    }
    if parts & security::DACL != 0 {
        put(&mut out, 16, &dacl.0);
    }
    out.0
}

/// Whether `name` matches the pattern `pattern` of QUERY_DIRECTORY, in
/// any case: `*` and `?`, and DOS's `<`, `>` and `"` (MS-FSA 2.1.4.4),
/// read here as `*`, `?` and `.`.
pub fn matches(pattern: &str, name: &str) -> bool {
    if pattern == "*" || pattern.is_empty() {
        return true;
    }
    let fold = |s: &str| -> Vec<char> { s.chars().flat_map(char::to_lowercase).collect() };
    let pattern: Vec<char> = fold(pattern)
        .into_iter()
        .map(|c| match c {
            '<' => '*',
            '>' => '?',
            '"' => '.',
            c => c,
        })
        .collect();
    let name = fold(name);
    // Glob matching, with backtracking to the last star.
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, n));
            p += 1;
        } else if let Some((sp, sn)) = star {
            p = sp + 1;
            n = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat() -> Stat {
        Stat {
            creation: 1,
            access: 2,
            write: 3,
            change: 4,
            size: 10,
            allocation: 4096,
            attributes: attribute::NORMAL,
            links: 1,
            inode: 99,
            device: 5,
            directory: false,
        }
    }

    #[test]
    fn entries_have_ms_fsccs_lengths() {
        for (class, fixed) in [
            (class::DIRECTORY, 64),
            (class::FULL_DIRECTORY, 68),
            (class::BOTH_DIRECTORY, 94),
            (class::ID_FULL_DIRECTORY, 80),
            (class::ID_BOTH_DIRECTORY, 104),
            (class::NAMES, 12),
        ] {
            assert_eq!(entry(class, &stat(), "ab").unwrap().len(), fixed + 4, "{class}");
        }
        assert_eq!(entry(60, &stat(), "ab"), Err(status::INVALID_INFO_CLASS));
        let id = entry(class::ID_BOTH_DIRECTORY, &stat(), "ab").unwrap();
        assert_eq!(wire::u64_at(&id, 96), Ok(99));
    }

    #[test]
    fn file_classes_have_ms_fsccs_lengths() {
        let open = Opened { name: "\\a", access: 0x1f01ff, position: 0, delete_pending: false };
        for (class, length) in [
            (class::BASIC, 40),
            (class::STANDARD, 24),
            (class::INTERNAL, 8),
            (class::NETWORK_OPEN, 56),
            (class::ATTRIBUTE_TAG, 8),
            (class::ALL, 100 + 4),
            (class::STREAM, 24 + 14),
            (class::COMPRESSION, 16),
            (class::ID, 24),
        ] {
            assert_eq!(file(class, &stat(), &open).unwrap().len(), length, "{class}");
        }
    }

    #[test]
    fn patterns_match_as_windows_does() {
        assert!(matches("*", "anything"));
        assert!(matches("*.TXT", "notes.txt"));
        assert!(!matches("*.txt", "notes.txt.bak"));
        assert!(matches("n?tes.*", "Notes.md"));
        assert!(matches("a*b*c", "aXXbYYc"));
        assert!(!matches("a*b*c", "aXXbYY"));
        assert!(matches("<.txt", "x.txt"));
        assert!(matches("exact", "EXACT"));
        assert!(!matches("exact", "exactly"));
    }

    #[test]
    fn a_descriptor_has_its_parts() {
        let all = descriptor(1000, 1000, 0o640, security::OWNER | security::GROUP | security::DACL);
        // Revision 1, self-relative with a DACL.
        assert_eq!(&all[..4], &[1, 0, 0x04, 0x80]);
        let owner = wire::u32_at(&all, 4).unwrap() as usize;
        assert_eq!(&all[owner..owner + 8], &[1, 2, 0, 0, 0, 0, 0, 22]);
        let only_owner = descriptor(1000, 1000, 0o640, security::OWNER);
        assert_eq!(wire::u32_at(&only_owner, 16), Ok(0));
        assert_eq!(rights(0, false), 0x0012_0080);
    }
}
