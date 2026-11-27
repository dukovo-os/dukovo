// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The system's names: the host's name, and what uname(2) says of the
//! system, its release and the machine.

use std::io;

use crate::{c_string, checked, field_bytes};

/// The longest host name, as Linux's HOST_NAME_MAX.
const HOST_NAME_MAX: usize = 64;

/// The host's name, as gethostname(2) gives it; bytes that are not UTF-8
/// are shown as U+FFFD.
///
/// # Errors
///
/// What gethostname fails with, which it does not on Dukovo or Linux.
pub fn host_name() -> io::Result<String> {
    let mut name = [0u8; HOST_NAME_MAX + 1];
    // SAFETY: gethostname writes at most `name.len()` bytes into `name`.
    checked(unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len()) })?;
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    Ok(String::from_utf8_lossy(&name[..end]).into_owned())
}

/// Sets the host's name.
///
/// # Errors
///
/// EPERM when the caller may not, EINVAL for a name longer than
/// HOST_NAME_MAX, and `ErrorKind::InvalidInput` for one with a NUL byte.
pub fn set_host_name(name: &str) -> io::Result<()> {
    // The C string only checks for a NUL; sethostname takes the length.
    c_string(name.as_bytes())?;
    // SAFETY: sethostname reads `name.len()` bytes of the name.
    checked(unsafe { libc::sethostname(name.as_ptr().cast(), name.len()) })?;
    Ok(())
}

/// What uname(2) says: the system's name, the host's, the kernel's
/// release and version, and the machine's kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Uname {
    /// `Dukovo`, or `Linux` on the build machine.
    pub system: String,
    /// The host's name.
    pub node: String,
    /// The kernel's release, such as `1.0.0-alpha.1`.
    pub release: String,
    /// The kernel's version: its build.
    pub version: String,
    /// The machine's kind, `x86_64`.
    pub machine: String,
}

/// What uname(2) says; bytes that are not UTF-8 are shown as U+FFFD.
///
/// # Errors
///
/// What uname fails with, which it does not on Dukovo or Linux.
pub fn uname() -> io::Result<Uname> {
    // SAFETY: all zeros is a valid utsname: arrays of characters.
    let mut names: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: uname fills the utsname it is given.
    checked(unsafe { libc::uname(&raw mut names) })?;
    let text = |field: &[libc::c_char]| String::from_utf8_lossy(&field_bytes(field)).into_owned();
    Ok(Uname {
        system: text(&names.sysname),
        node: text(&names.nodename),
        release: text(&names.release),
        version: text(&names.version),
        machine: text(&names.machine),
    })
}

#[cfg(test)]
mod tests {
    use super::{host_name, uname};

    #[test]
    fn the_host_name_is_uname_s_node() {
        assert_eq!(host_name().unwrap(), uname().unwrap().node);
    }
}
