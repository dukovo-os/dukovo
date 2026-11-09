// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! eject(1): unmounts a volume at `/volumes`, writing out what it holds
//! first, so that its disk can be pulled out (ADR 0163), as util-linux's
//! eject does with a mount point or a device. A disk's name unmounts each
//! of its partitions'.
//!
//! On a drive whose medium comes and goes, such as a card reader's slot
//! (ADR 0430), it unmounts every volume of the medium, as util-linux's
//! eject unmounts every partition of the disk, and then ejects the
//! medium (radix-block(2)'s EJECT): a tray opens, and a card can be taken
//! out. The drive stays, empty. A drive's name ejects it with nothing
//! mounted from it too.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use radix_abi::block;
use radix_abi::errors::{File as FileError, Kernel};

use crate::args::{Arg, Args};
use crate::blockdev;
use crate::df;
use crate::probe;
use crate::proc::Mount;
use crate::sys;
use crate::util::{self, Fail, usage};
use crate::volumed;

/// The volumes `what` names: a label or name under `/volumes`, a path
/// there, or a disk or partition by its name or its path in `/devices`.
pub fn named<'a>(mounts: &'a [Mount], what: &str) -> Vec<&'a Mount> {
    let device = what.strip_prefix("/devices/").unwrap_or(what);
    let path =
        if what.starts_with('/') { PathBuf::from(what) } else { Path::new("/volumes").join(what) };
    let by_path: Vec<&Mount> = mounts.iter().filter(|m| df::mount_path(m) == path).collect();
    if !by_path.is_empty() {
        return by_path;
    }
    mounts.iter().filter(|m| m.source == device || probe::disk_of(&m.source) == device).collect()
}

/// The disk that the disk, partition or unlocked BitLocker volume `name`
/// is on: `sdb` for `sdb1` and `sdb1-unlocked`.
fn disk_of(name: &str) -> &str {
    let name = name.strip_suffix("-unlocked").unwrap_or(name);
    match probe::block_text(name, "partition") {
        Some(_) => probe::disk_of(name),
        None => name,
    }
}

/// Whether the disk `name` is a drive whose medium comes and goes.
fn is_drive(name: &str) -> bool {
    probe::block_text(name, "removable").as_deref() == Some("1")
}

/// Ejects the medium of the drive `disk`.
fn eject(disk: &str) -> Result<(), String> {
    let device = File::open(format!("/devices/{disk}")).map_err(|e| util::strip_os_error(&e))?;
    match blockdev::control(&device, block::EJECT, &mut []) {
        Ok(()) => Ok(()),
        Err(status) if FileError::from_status(status) == Some(FileError::InUse) => {
            Err("it, or a partition of it, is in use outside /volumes".into())
        }
        // Not a drive after all: there is nothing to eject.
        Err(status) if Kernel::from_status(status) == Some(Kernel::NotSupported) => Ok(()),
        Err(status) => Err(util::strip_os_error(&sys::io_error(status))),
    }
}

pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let (mut verbose, mut what) = (false, None::<String>);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('v') => verbose = true,
            Arg::Long(long) if long == "verbose" => verbose = true,
            Arg::Operand(operand) if what.is_none() => {
                what = Some(operand.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(what) = what else { return Err(usage("no volume given".into())) };
    let mounts = volumed::volume_mounts().map_err(|e| util::failed(&"/processes/mounts", e))?;
    let mut chosen = named(&mounts, &what);
    let mut disks: Vec<&str> = chosen.iter().map(|mount| disk_of(&mount.source)).collect();
    // A drive named with nothing of it mounted is ejected all the same.
    let device = what.strip_prefix("/devices/").unwrap_or(&what);
    if chosen.is_empty() && is_drive(device) {
        disks.push(device);
    }
    if disks.is_empty() {
        return Err(util::fail(format!("{what}: no volume at /volumes")));
    }
    disks.sort_unstable();
    disks.dedup();
    let drives: Vec<&str> = disks.into_iter().filter(|disk| is_drive(disk)).collect();
    // A drive's medium goes whole: every volume on it is unmounted.
    for mount in &mounts {
        let on_drive = drives.contains(&disk_of(&mount.source));
        if on_drive && !chosen.iter().any(|chose| std::ptr::eq(*chose, mount)) {
            chosen.push(mount);
        }
    }
    let mut out = io::stdout().lock();
    for mount in chosen {
        let path = df::mount_path(mount);
        if verbose {
            writeln!(out, "eject: unmounting {} (/devices/{})", path.display(), mount.source)?;
        }
        volumed::unmount(&path)
            .map_err(|why| util::fail(format!("{}: cannot unmount: {why}", path.display())))?;
    }
    for drive in drives {
        if verbose {
            writeln!(out, "eject: ejecting the medium of /devices/{drive}")?;
        }
        eject(drive).map_err(|why| util::fail(format!("{drive}: cannot eject: {why}")))?;
    }
    Ok(0)
}
