// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What smbd asks of the system in the terms Windows' requests use: the
//! machine's name, a filesystem's sizes, a file's times, and the names
//! and numbers of users and groups, on sysapi's calls (ADR 0730).

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use sysapi::fs::{self, Link, Time};
use sysapi::{system, users};

use crate::info::Volume;

/// The machine's name.
pub fn hostname() -> String {
    system::host_name().unwrap_or_else(|_| "localhost".into())
}

/// What the filesystem holding `path` is.
pub fn volume(path: &Path) -> io::Result<Volume> {
    let st = fs::statvfs(path)?;
    let unit = if st.f_frsize > 0 { st.f_frsize } else { st.f_bsize };
    let device = std::fs::metadata(path)?.dev();
    Ok(Volume {
        blocks: st.f_blocks as u64,
        available: st.f_bavail as u64,
        free: st.f_bfree as u64,
        block_size: u32::try_from(unit).unwrap_or(4096),
        serial: (device ^ (device >> 32)) as u32,
        name_max: u32::try_from(st.f_namemax).unwrap_or(255).max(1),
    })
}

/// Sets `path`'s access and modification times, each as seconds and
/// nanoseconds; `None` leaves one as it is.
pub fn set_times(
    path: &Path,
    access: Option<(i64, i64)>,
    write: Option<(i64, i64)>,
) -> io::Result<()> {
    let time = |time: Option<(i64, i64)>| match time {
        None => Ok(Time::Keep),
        Some((seconds, nanoseconds)) => u32::try_from(nanoseconds)
            .map(|nanoseconds| Time::At { seconds, nanoseconds })
            .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL)),
    };
    fs::set_times(path, time(access)?, time(write)?, Link::Follow)
}

/// User `uid`'s name, if it has one.
pub fn user_name(uid: u32) -> Option<String> {
    users::user_by_id(uid).ok().flatten().map(|user| user.name)
}

/// Group `gid`'s name, if it has one.
pub fn group_name(gid: u32) -> Option<String> {
    users::group_by_id(gid).ok().flatten().map(|group| group.name)
}

/// The number of the user called `name`, if there is one.
pub fn user_id(name: &str) -> Option<u32> {
    users::user_by_name(name).ok().flatten().map(|user| user.uid)
}

/// The number of the group called `name`, if there is one.
pub fn group_id(name: &str) -> Option<u32> {
    users::group_by_name(name).ok().flatten().map(|group| group.gid)
}
