// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! POSIX access lists (ADR 0256) as Linux keeps them: the extended
//! attributes `system.posix_acl_access` and `system.posix_acl_default`,
//! in `posix_acl_xattr`'s form, read, changed and written as libacl does.
//! getfacl(1) and setfacl(1) show and change them, and smbd(8) shows
//! them to Windows as security descriptors (ADR 0229).

use std::ffi::{CString, c_char, c_int, c_void};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// The attribute of the list that decides.
pub const ACCESS: &str = "system.posix_acl_access";
/// The attribute of a directory's list for what is made in it.
pub const DEFAULT: &str = "system.posix_acl_default";

// The C library's extended attribute calls (sys/xattr.h), which the
// libc crate does not declare for Radix.
unsafe extern "C" {
    fn getxattr(path: *const c_char, name: *const c_char, value: *mut c_void, size: usize)
    -> isize;
    fn lgetxattr(
        path: *const c_char,
        name: *const c_char,
        value: *mut c_void,
        size: usize,
    ) -> isize;
    fn setxattr(
        path: *const c_char,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        flags: c_int,
    ) -> c_int;
    fn removexattr(path: *const c_char, name: *const c_char) -> c_int;
}

/// What an entry names, in the order of a list: Linux's tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tag {
    /// The owner's: `user::`.
    UserObj = 0x01,
    /// A named user's: `user:name:`.
    User = 0x02,
    /// The file's group's: `group::`.
    GroupObj = 0x04,
    /// A named group's: `group:name:`.
    Group = 0x08,
    /// The mask, which limits the named entries and the group's.
    Mask = 0x10,
    /// Everyone else's.
    Other = 0x20,
}

impl Tag {
    /// The tag of an entry's bits, if they are one.
    #[must_use]
    pub fn from_bits(bits: u16) -> Option<Tag> {
        Some(match bits {
            0x01 => Tag::UserObj,
            0x02 => Tag::User,
            0x04 => Tag::GroupObj,
            0x08 => Tag::Group,
            0x10 => Tag::Mask,
            0x20 => Tag::Other,
            _ => return None,
        })
    }

    /// Whether it has a number: a named user's or group's.
    pub fn named(self) -> bool {
        matches!(self, Tag::User | Tag::Group)
    }

    /// Whether the mask limits it: the group class.
    pub fn masked(self) -> bool {
        matches!(self, Tag::User | Tag::GroupObj | Tag::Group)
    }

    /// Its word in getfacl's text.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Tag::UserObj | Tag::User => "user",
            Tag::GroupObj | Tag::Group => "group",
            Tag::Mask => "mask",
            Tag::Other => "other",
        }
    }
}

/// An entry: what it names (with its number if named) and its read (4),
/// write (2) and execute (1) permissions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// What it names.
    pub tag: Tag,
    /// The user's or group's number, or [`NO_ID`].
    pub id: u32,
    /// Its permissions: read 4, write 2, execute 1.
    pub perm: u8,
}

/// Linux's number for an entry that names no one.
pub const NO_ID: u32 = u32::MAX;

/// An access list, its entries in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Acl {
    /// Its entries, in order.
    pub entries: Vec<Entry>,
}

impl Acl {
    /// The list a mode's permission bits say.
    pub fn from_mode(mode: u32) -> Acl {
        let perm = |shift: u32| ((mode >> shift) & 7) as u8;
        Acl {
            entries: vec![
                Entry { tag: Tag::UserObj, id: NO_ID, perm: perm(6) },
                Entry { tag: Tag::GroupObj, id: NO_ID, perm: perm(3) },
                Entry { tag: Tag::Other, id: NO_ID, perm: perm(0) },
            ],
        }
    }

    /// The list in an attribute's value; none for one with no entries or
    /// one that does not read.
    pub fn from_value(value: &[u8]) -> Option<Acl> {
        if value.len() < 4
            || !(value.len() - 4).is_multiple_of(8)
            || value[..4] != 2u32.to_le_bytes()
        {
            return None;
        }
        let mut acl = Acl::default();
        for raw in value[4..].as_chunks::<8>().0 {
            let tag = Tag::from_bits(u16::from_le_bytes([raw[0], raw[1]]))?;
            let perm = u16::from_le_bytes([raw[2], raw[3]]);
            let id = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
            let id = if tag.named() { id } else { NO_ID };
            acl.set(Entry { tag, id, perm: (perm & 7) as u8 });
        }
        (!acl.entries.is_empty()).then_some(acl)
    }

    /// Its attribute value.
    pub fn to_value(&self) -> Vec<u8> {
        let mut value = 2u32.to_le_bytes().to_vec();
        for e in &self.entries {
            value.extend_from_slice(&(e.tag as u16).to_le_bytes());
            value.extend_from_slice(&u16::from(e.perm).to_le_bytes());
            value.extend_from_slice(&e.id.to_le_bytes());
        }
        value
    }

    /// The entry of `tag` and `id`, if there is one.
    #[must_use]
    pub fn get(&self, tag: Tag, id: u32) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag && e.id == id)
    }

    /// Sets an entry, in its place.
    pub fn set(&mut self, entry: Entry) {
        match self.entries.binary_search_by_key(&(entry.tag, entry.id), |e| (e.tag, e.id)) {
            Ok(i) => self.entries[i] = entry,
            Err(i) => self.entries.insert(i, entry),
        }
    }

    /// Takes the entry of `tag` and `id` away.
    pub fn remove(&mut self, tag: Tag, id: u32) {
        self.entries.retain(|e| !(e.tag == tag && e.id == id));
    }

    /// The mask's permissions, if it has one.
    #[must_use]
    pub fn mask(&self) -> Option<u8> {
        self.get(Tag::Mask, NO_ID).map(|e| e.perm)
    }

    /// Whether it names a user or group, and so needs a mask.
    pub fn has_named(&self) -> bool {
        self.entries.iter().any(|e| e.tag.named())
    }

    /// Whether it says no more than a mode does.
    pub fn is_base(&self) -> bool {
        self.entries.iter().all(|e| matches!(e.tag, Tag::UserObj | Tag::GroupObj | Tag::Other))
    }

    /// Its owner's, group's and others' entries alone, as `setfacl -b`
    /// leaves them.
    pub fn base(&self) -> Acl {
        Acl {
            entries: self
                .entries
                .iter()
                .filter(|e| matches!(e.tag, Tag::UserObj | Tag::GroupObj | Tag::Other))
                .copied()
                .collect(),
        }
    }

    /// The mask that lets every entry it limits have what it names: all
    /// their permissions together (libacl's `acl_calc_mask`).
    pub fn calc_mask(&self) -> u8 {
        self.entries.iter().filter(|e| e.tag.masked()).fold(0, |m, e| m | e.perm)
    }

    /// The first entry missing for a whole list, counted from 1 as
    /// setfacl counts them; none if it is whole.
    pub fn missing(&self) -> Option<usize> {
        let has = |tag| self.entries.iter().any(|e| e.tag == tag);
        if !has(Tag::UserObj) {
            return Some(1);
        }
        if !has(Tag::GroupObj) {
            return Some(2);
        }
        if !has(Tag::Other) {
            return Some(3);
        }
        None
    }
}

/// A file's attribute `name` read, following a symbolic link unless
/// `physical`: none if it has none, or its filesystem keeps none.
///
/// # Errors
///
/// The call's, but for ENODATA and EOPNOTSUPP, which are none.
pub fn read_attribute(path: &Path, name: &str, physical: bool) -> io::Result<Option<Vec<u8>>> {
    let c_path = c_string(path.as_os_str().as_bytes())?;
    let c_name = c_string(name.as_bytes())?;
    let mut buffer = vec![0u8; 256];
    loop {
        // SAFETY: the path and name are C strings, and the buffer is as
        // long as the size given.
        let n = unsafe {
            let get = if physical { lgetxattr } else { getxattr };
            get(c_path.as_ptr(), c_name.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len())
        };
        if n >= 0 {
            buffer.truncate(n as usize);
            return Ok(Some(buffer));
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ENODATA | libc::EOPNOTSUPP) => return Ok(None),
            Some(libc::ERANGE) if buffer.len() < 65536 => buffer.resize(buffer.len() * 4, 0),
            _ => return Err(error),
        }
    }
}

fn c_string(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}

/// A file's access list, from its attribute or its mode.
///
/// # Errors
///
/// Reading the attribute's.
pub fn access_list(path: &Path, mode: u32, physical: bool) -> io::Result<Acl> {
    Ok(read_attribute(path, ACCESS, physical)?
        .and_then(|v| Acl::from_value(&v))
        .unwrap_or_else(|| Acl::from_mode(mode)))
}

/// A directory's default list, if it has one.
///
/// # Errors
///
/// Reading the attribute's.
pub fn default_list(path: &Path, physical: bool) -> io::Result<Option<Acl>> {
    Ok(read_attribute(path, DEFAULT, physical)?.and_then(|v| Acl::from_value(&v)))
}

/// Sets a file's access list. One the mode says all of is set as a mode
/// where the filesystem keeps no lists, as libacl does.
///
/// # Errors
///
/// Setting the attribute's, or the mode's.
pub fn write_access(path: &Path, acl: &Acl, mode: u32) -> io::Result<()> {
    let c_path = c_string(path.as_os_str().as_bytes())?;
    let c_name = c_string(ACCESS.as_bytes())?;
    let value = acl.to_value();
    // SAFETY: C strings, and the value's bytes with their length.
    let r = unsafe {
        setxattr(c_path.as_ptr(), c_name.as_ptr(), value.as_ptr().cast(), value.len(), 0)
    };
    if r == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EOPNOTSUPP) && acl.is_base() {
        let perm = |tag| u32::from(acl.get(tag, NO_ID).map_or(0, |e| e.perm));
        let bits = (mode & 0o7000)
            | (perm(Tag::UserObj) << 6)
            | (perm(Tag::GroupObj) << 3)
            | perm(Tag::Other);
        return std::fs::set_permissions(path, std::fs::Permissions::from_mode(bits));
    }
    Err(error)
}

/// Sets a directory's default list, or takes it away given none.
///
/// # Errors
///
/// Setting or removing the attribute's.
pub fn write_default(path: &Path, acl: Option<&Acl>) -> io::Result<()> {
    let c_path = c_string(path.as_os_str().as_bytes())?;
    let c_name = c_string(DEFAULT.as_bytes())?;
    // SAFETY: C strings, and the value's bytes with their length.
    let r = unsafe {
        match acl {
            Some(acl) => {
                let value = acl.to_value();
                setxattr(c_path.as_ptr(), c_name.as_ptr(), value.as_ptr().cast(), value.len(), 0)
            }
            None => removexattr(c_path.as_ptr(), c_name.as_ptr()),
        }
    };
    let error = io::Error::last_os_error();
    if r == 0
        || (acl.is_none() && matches!(error.raw_os_error(), Some(libc::ENODATA | libc::EOPNOTSUPP)))
    {
        return Ok(());
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(entries: &[(Tag, u32, u8)]) -> Acl {
        Acl { entries: entries.iter().map(|&(tag, id, perm)| Entry { tag, id, perm }).collect() }
    }

    #[test]
    fn values_go_both_ways() {
        let acl = list(&[
            (Tag::UserObj, NO_ID, 6),
            (Tag::User, 1000, 4),
            (Tag::GroupObj, NO_ID, 4),
            (Tag::Mask, NO_ID, 4),
            (Tag::Other, NO_ID, 0),
        ]);
        let value = acl.to_value();
        assert_eq!(value.len(), 4 + 5 * 8);
        assert_eq!(&value[..12], &[2, 0, 0, 0, 1, 0, 6, 0, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(Acl::from_value(&value), Some(acl));
        // Another version, a cut entry, an unknown tag, or none at all.
        assert_eq!(Acl::from_value(&[1, 0, 0, 0]), None);
        assert_eq!(Acl::from_value(&value[..value.len() - 1]), None);
        assert_eq!(Acl::from_value(&[2, 0, 0, 0, 0x40, 0, 7, 0, 0, 0, 0, 0]), None);
        assert_eq!(Acl::from_value(&[2, 0, 0, 0]), None);
    }

    #[test]
    fn entries_keep_their_order_and_the_mask_covers_them() {
        let mut acl = Acl::from_mode(0o640);
        assert!(acl.is_base() && !acl.has_named() && acl.mask().is_none());
        acl.set(Entry { tag: Tag::Group, id: 50, perm: 2 });
        acl.set(Entry { tag: Tag::User, id: 7, perm: 1 });
        let tags: Vec<_> = acl.entries.iter().map(|e| (e.tag, e.id)).collect();
        assert_eq!(
            tags,
            [
                (Tag::UserObj, NO_ID),
                (Tag::User, 7),
                (Tag::GroupObj, NO_ID),
                (Tag::Group, 50),
                (Tag::Other, NO_ID)
            ]
        );
        assert_eq!(acl.calc_mask(), 4 | 2 | 1);
        assert_eq!(acl.base(), Acl::from_mode(0o640));
        acl.remove(Tag::UserObj, NO_ID);
        assert_eq!(acl.missing(), Some(1));
    }

    #[test]
    fn lists_are_kept_where_the_filesystem_keeps_them() {
        let dir = std::env::temp_dir().join(format!("acl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut acl = Acl::from_mode(0o750);
        acl.set(Entry { tag: Tag::User, id: 4242, perm: 5 });
        acl.set(Entry { tag: Tag::Mask, id: NO_ID, perm: 5 });
        match write_access(&dir, &acl, 0o750) {
            Err(err) if err.raw_os_error() == Some(libc::EOPNOTSUPP) => {
                eprintln!("this filesystem keeps no access lists");
            }
            done => {
                done.unwrap();
                assert_eq!(access_list(&dir, 0, false).unwrap(), acl);
                write_default(&dir, Some(&acl)).unwrap();
                assert_eq!(default_list(&dir, true).unwrap(), Some(acl));
                write_default(&dir, None).unwrap();
                write_default(&dir, None).unwrap();
                assert_eq!(default_list(&dir, false).unwrap(), None);
            }
        }
        // A list the mode says all of is a mode anywhere.
        write_access(&dir, &Acl::from_mode(0o700), 0o755).unwrap();
        assert_eq!(access_list(&dir, 0o700, false).unwrap(), Acl::from_mode(0o700));
        assert_eq!(
            read_attribute(&dir.join("missing"), ACCESS, false).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
