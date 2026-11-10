// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! device(8): adds disks to a Btrfs filesystem and removes them, as
//! Linux's `btrfs device add` and `btrfs device remove` do (ADR 0159),
//! and grows a disk of it to its partition's new size, as `btrfs
//! filesystem resize` does (ADR 0327), through the directory interface
//! (radix-directory(2)).

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;

use radix_abi::errors::{File as FileError, Kernel};
use radix_abi::{Status, directory};

use crate::args::{Arg, Args};
use crate::sys::{self, SYS};
use crate::util::{self, Fail};

/// Where a disk's first superblock is, and its magic and device ID in it.
const SUPERBLOCK: u64 = 0x10000;
const MAGIC_AT: usize = 0x40;
const DEVID_AT: usize = 0xc9;
const MAGIC: &[u8; 8] = b"_BHRfS_M";

/// The superblock of the disk `file`, if it holds a Btrfs.
fn superblock(file: &File) -> Option<[u8; 0x100]> {
    let mut bytes = [0; 0x100];
    file.read_exact_at(&mut bytes, SUPERBLOCK).ok()?;
    (&bytes[MAGIC_AT..MAGIC_AT + 8] == MAGIC).then_some(bytes)
}

/// What a failed call on `path` says, as btrfs-progs words it.
fn message(status: Status, what: &str, path: &str) -> String {
    match (Kernel::from_status(status), FileError::from_status(status)) {
        (Some(Kernel::NotSupported), _) => format!("{path}: not a btrfs filesystem"),
        (_, Some(FileError::NotFound)) => format!("{what}: not a valid devid for '{path}'"),
        (_, Some(FileError::InUse)) => format!("{what} is mounted"),
        (_, Some(FileError::NoSpace)) => format!("{what}: larger than its partition"),
        (Some(Kernel::InvalidArgument), _) => format!(
            "error removing device '{what}': unable to go below the disks its chunks' profiles \
             need"
        ),
        _ => format!("{what}: {}", util::strip_os_error(&sys::io_error(status))),
    }
}

/// Adds the disk `disk` to the filesystem whose directory is in `dir`.
fn add(dir: &File, disk: &OsString, force: bool, path: &str) -> Result<(), Fail> {
    let shown = disk.to_string_lossy();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(disk)
        .map_err(|e| util::failed(&shown, e))?;
    if !force && superblock(&file).is_some() {
        return Err(util::fail(format!(
            "{shown} appears to contain an existing filesystem (btrfs)\nUse the -f option to \
             force overwrite of {shown}"
        )));
    }
    let (dir_slot, disk_slot) = (sys::slot_of(dir.as_raw_fd()), sys::slot_of(file.as_raw_fd()));
    directory::add_disk(&SYS, dir_slot, disk_slot)
        .map_err(|status| util::fail(message(status, &shown, path)))?;
    Ok(())
}

/// Removes the disk `disk`, a device ID or a disk of the filesystem whose
/// directory is in `dir`.
fn remove(dir: &File, disk: &OsString, path: &str) -> Result<(), Fail> {
    let shown = disk.to_string_lossy();
    let devid = match shown.parse::<u64>() {
        Ok(devid) => devid,
        Err(_) if shown == "missing" => {
            return Err(util::fail("name the missing disk by its devid".into()));
        }
        Err(_) => {
            let file = File::open(disk).map_err(|e| util::failed(&shown, e))?;
            let bytes = superblock(&file)
                .ok_or_else(|| util::fail(format!("{shown}: not a btrfs device")))?;
            u64::from_le_bytes(bytes[DEVID_AT..DEVID_AT + 8].try_into().unwrap_or([0; 8]))
        }
    };
    directory::remove_disk(&SYS, sys::slot_of(dir.as_raw_fd()), devid)
        .map_err(|status| util::fail(message(status, &shown, path)))?;
    Ok(())
}

/// What `resize` was asked for: `[DEVID:]max` or `[DEVID:]SIZE`, as
/// btrfs-progs takes it; device ID 1 if none is given. Size 0 is `max`.
/// Shrinking, and growing by an amount (`+SIZE`), are not done.
fn resize_amount(text: &str) -> Result<(u64, u64), Fail> {
    let (devid, size) = match text.split_once(':') {
        Some((devid, size)) => {
            let devid = devid
                .parse::<u64>()
                .map_err(|_| util::usage(format!("'{devid}' is not a device ID")))?;
            (devid, size)
        }
        None => (1, text),
    };
    if size.starts_with(['+', '-']) {
        return Err(util::usage(format!(
            "'{size}': say max or the size the disk is to have; a disk is grown only, to a size"
        )));
    }
    let bytes = if size == "max" {
        0
    } else {
        util::parse_size(size)
            .filter(|&b| b > 0)
            .ok_or_else(|| util::usage(format!("'{size}' is neither max nor a size")))?
    };
    Ok((devid, bytes))
}

/// `resize`: grows the disk `amount` names of the filesystem whose
/// directory is in `dir`, and says to what size.
fn resize(dir: &File, amount: &str, path: &str) -> Result<(), Fail> {
    let (devid, size) = resize_amount(amount)?;
    let now = directory::resize_disk(&SYS, sys::slot_of(dir.as_raw_fd()), devid, size)
        .map_err(|status| util::fail(message(status, &format!("devid {devid}"), path)))?;
    println!("Resized device id {devid} of '{path}' to {}", disk::size::human(now.size));
    Ok(())
}

/// device(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let usage = || util::usage("say add, remove or resize, the disks or size, and a path".into());
    let Some(action) = args.subcommand() else { return Err(usage()) };
    let adding = match action.to_str() {
        Some("add") => true,
        Some("remove" | "delete") => false,
        Some("resize") => {
            let mut operands = Vec::new();
            while let Some(arg) = args.next()? {
                match arg {
                    Arg::Operand(operand) => operands.push(operand),
                    other => return Err(args.unknown(&other)),
                }
            }
            let [amount, path] = operands.as_slice() else { return Err(usage()) };
            let path = PathBuf::from(path);
            let shown = path.display().to_string();
            let dir = sys::open_control(&path).map_err(|e| util::failed(&shown, e))?;
            resize(&dir, &amount.to_string_lossy(), &shown)?;
            return Ok(0);
        }
        _ => return Err(usage()),
    };
    let (mut force, mut operands) = (false, Vec::new());
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('f') if adding => force = true,
            Arg::Long(long) if adding && long == "force" => force = true,
            // A new disk is not discarded first, as -K asks.
            Arg::Short('K') if adding => {}
            Arg::Long(long) if adding && long == "nodiscard" => {}
            Arg::Operand(operand) => operands.push(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    let Some((path, disks)) = operands.split_last() else { return Err(usage()) };
    if disks.is_empty() {
        return Err(usage());
    }
    let path = PathBuf::from(path);
    let shown = path.display().to_string();
    let dir = sys::open_control(&path).map_err(|e| util::failed(&shown, e))?;
    for disk in disks {
        if adding {
            add(&dir, disk, force, &shown)?;
        } else {
            remove(&dir, disk, &shown)?;
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resize_names_a_disk_and_max_or_a_size() {
        assert_eq!(resize_amount("max").unwrap(), (1, 0));
        assert_eq!(resize_amount("2:max").unwrap(), (2, 0));
        assert_eq!(resize_amount("3:20G").unwrap(), (3, 20 << 30));
        assert_eq!(resize_amount("1048576").unwrap(), (1, 1 << 20));
        for refused in ["+1G", "-1G", "2:-1G", "x:max", "0", "most", ""] {
            assert!(resize_amount(refused).is_err(), "{refused}");
        }
    }
}
