// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The accounts' and groups' entries, as the C library reads them
//! (getpwnam_r(3), getgrgid_r(3)): by number or by name, copied out so
//! that nothing points into the C library's buffers.

use std::ffi::{CStr, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use libc::c_char;

use crate::c_string;

/// An account's entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    /// Its name.
    pub name: String,
    /// Its user id.
    pub uid: u32,
    /// Its login group's id.
    pub gid: u32,
    /// Its home folder.
    pub home: PathBuf,
    /// The shell it signs in to.
    pub shell: PathBuf,
}

/// A group's entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// Its name.
    pub name: String,
    /// Its group id.
    pub gid: u32,
}

/// The largest buffer a lookup grows to for an entry's strings; an entry
/// longer than this is a broken file.
const BUFFER_MAX: usize = 1 << 20;

/// Runs `lookup`, a reentrant call of the C library, with an entry and a
/// buffer for its strings, growing the buffer while the call says it is
/// too small (ERANGE). The entry is filled when the call found one.
fn look_up<T, R>(
    lookup: impl Fn(*mut T, *mut c_char, usize, *mut *mut T) -> libc::c_int,
    read: impl FnOnce(&T) -> io::Result<R>,
) -> io::Result<Option<R>> {
    // SAFETY: all zeros is a valid passwd or group: numbers and null
    // pointers, which the call fills.
    let mut entry: T = unsafe { std::mem::zeroed() };
    let mut buffer = vec![0 as c_char; 1024];
    loop {
        let mut found: *mut T = std::ptr::null_mut();
        match lookup(&raw mut entry, buffer.as_mut_ptr(), buffer.len(), &raw mut found) {
            0 if found.is_null() => return Ok(None),
            0 => return read(&entry).map(Some),
            libc::ERANGE if buffer.len() < BUFFER_MAX => buffer.resize(buffer.len() * 4, 0),
            errno => return Err(io::Error::from_raw_os_error(errno)),
        }
    }
}

/// The text of a string the C library filled, which lives in the lookup's
/// buffer.
///
/// # Safety
///
/// `text` is null, or points at a C string that lives through the call.
unsafe fn text_of(text: *const c_char) -> io::Result<String> {
    if text.is_null() {
        return Ok(String::new());
    }
    // SAFETY: the caller's contract.
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    String::from_utf8(bytes.to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "an account's name is not UTF-8"))
}

/// The path of a string the C library filled.
///
/// # Safety
///
/// As [`text_of`]'s.
unsafe fn path_of(text: *const c_char) -> PathBuf {
    if text.is_null() {
        return PathBuf::new();
    }
    // SAFETY: the caller's contract.
    PathBuf::from(OsStr::from_bytes(unsafe { CStr::from_ptr(text) }.to_bytes()))
}

fn user_of(entry: &libc::passwd) -> io::Result<User> {
    // SAFETY: the strings of an entry the C library filled point into the
    // lookup's buffer, which outlives this read of them.
    unsafe {
        Ok(User {
            name: text_of(entry.pw_name)?,
            uid: entry.pw_uid,
            gid: entry.pw_gid,
            home: path_of(entry.pw_dir),
            shell: path_of(entry.pw_shell),
        })
    }
}

fn group_of(entry: &libc::group) -> io::Result<Group> {
    // SAFETY: as in `user_of`.
    Ok(Group { name: unsafe { text_of(entry.gr_name) }?, gid: entry.gr_gid })
}

/// The account with user id `uid`, if there is one.
///
/// # Errors
///
/// EIO or ENOMEM when the account files cannot be read, and
/// `ErrorKind::InvalidData` for a name that is not UTF-8.
pub fn user_by_id(uid: u32) -> io::Result<Option<User>> {
    look_up(
        // SAFETY: getpwuid_r fills the entry and its strings in the buffer
        // of the length given, and sets `found`.
        |entry, buffer, length, found| unsafe {
            libc::getpwuid_r(uid, entry, buffer, length, found)
        },
        user_of,
    )
}

/// The account named `name`, if there is one.
///
/// # Errors
///
/// As [`user_by_id`]'s, and `ErrorKind::InvalidInput` for a name with a
/// NUL byte.
pub fn user_by_name(name: &str) -> io::Result<Option<User>> {
    let name = c_string(name.as_bytes())?;
    look_up(
        // SAFETY: as in `user_by_id`; the name is a C string.
        |entry, buffer, length, found| unsafe {
            libc::getpwnam_r(name.as_ptr(), entry, buffer, length, found)
        },
        user_of,
    )
}

/// The group with group id `gid`, if there is one.
///
/// # Errors
///
/// As [`user_by_id`]'s.
pub fn group_by_id(gid: u32) -> io::Result<Option<Group>> {
    look_up(
        // SAFETY: getgrgid_r fills the entry and its strings in the buffer
        // of the length given, and sets `found`.
        |entry, buffer, length, found| unsafe {
            libc::getgrgid_r(gid, entry, buffer, length, found)
        },
        group_of,
    )
}

/// The group named `name`, if there is one.
///
/// # Errors
///
/// As [`user_by_name`]'s.
pub fn group_by_name(name: &str) -> io::Result<Option<Group>> {
    let name = c_string(name.as_bytes())?;
    look_up(
        // SAFETY: as in `group_by_id`; the name is a C string.
        |entry, buffer, length, found| unsafe {
            libc::getgrnam_r(name.as_ptr(), entry, buffer, length, found)
        },
        group_of,
    )
}

#[cfg(test)]
mod tests {
    use super::{group_by_id, group_by_name, user_by_id, user_by_name};

    #[test]
    fn root_is_found_by_number_and_by_name() {
        let root = user_by_id(0).unwrap().unwrap();
        assert_eq!(root.name, "root");
        assert_eq!(root.uid, 0);
        assert_eq!(user_by_name("root").unwrap(), Some(root));
        let group = group_by_id(0).unwrap().unwrap();
        assert_eq!(group_by_name(&group.name).unwrap(), Some(group));
    }

    #[test]
    fn an_account_that_is_not_there_is_none() {
        assert_eq!(user_by_name("no-such-account-here").unwrap(), None);
        assert_eq!(group_by_name("no-such-group-here").unwrap(), None);
        assert!(user_by_name("a\0b").is_err());
    }
}
