// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! mount(8) and umount(8), as Linux's, for the filesystems Dukovo has: a
//! disk's (FAT, exFAT, NTFS, Btrfs, ext4, UDF and ISO 9660, whichever it
//! holds when `-t` names none), a share of a Windows network, which
//! smb(1) mounts and serves (`-t smb` or `-t cifs`, or a source that
//! starts with `//`; ADR 0227), and a folder a Unix machine shares over
//! NFS, which nfs(1) mounts and serves (`-t nfs` or `-t nfs4`, or a
//! source `server:/path`; ADR 0344). Without a source and a folder, mount lists
//! the mounts as Linux's does. umount unmounts the filesystem at a folder,
//! or the one a source is mounted from. Volumes that come and go are
//! volumed(8)'s, at /volumes; these are for the rest.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use radix_abi::directory::{self, MountFlags};
use radix_abi::errors::{File as FileError, Kernel};
use radix_abi::{Status, startup};

use crate::args::{Arg, Args};
use crate::df;
use crate::proc::{self, Mount};
use crate::sys::{self, SYS};
use crate::util::{self, Fail, usage};
use crate::volumed::KINDS;

/// The program that mounts and serves shares.
const SMB: &str = "/system/commands/smb";

/// The kinds that name a share, which smb(1) mounts.
const SHARES: &[&str] = &["smb", "smb3", "cifs"];

/// The program that mounts and serves folders shared over NFS.
const NFS: &str = "/system/commands/nfs";

/// The kinds that name a folder shared over NFS, which nfs(1) mounts.
const EXPORTS: &[&str] = &["nfs", "nfs4"];

/// Whether `source` names a share: `//server/share`, `\\server\share`
/// or `smb://server/share`.
fn is_share(source: &[u8]) -> bool {
    source.starts_with(b"//") || source.starts_with(b"\\\\") || source.starts_with(b"smb://")
}

/// Whether `source` names a folder shared over NFS: `server:/path`,
/// `[v6]:/path` or `nfs://server/path`.
fn is_export(source: &[u8]) -> bool {
    if source.starts_with(b"nfs://") {
        return true;
    }
    match source.windows(2).position(|pair| pair == b":/") {
        Some(at) => at > 0 && !source[..at].contains(&b'/'),
        None => false,
    }
}

pub fn mount(args: &mut Args) -> Result<i32, Fail> {
    let (mut kinds, mut options, mut access, mut labels) = (None, Vec::new(), None, false);
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('t') => kinds = Some(args.text()?),
            Arg::Long(long) if long == "types" => kinds = Some(args.text()?),
            Arg::Short('o') => options.push(args.text()?),
            Arg::Long(long) if long == "options" => options.push(args.text()?),
            Arg::Short('r') => access = Some("ro"),
            Arg::Long(long) if long == "read-only" => access = Some("ro"),
            Arg::Short('w') => access = Some("rw"),
            Arg::Long(long) if long == "rw" || long == "read-write" => access = Some("rw"),
            Arg::Short('l') => labels = true,
            Arg::Operand(operand) => operands.push(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    match operands.as_slice() {
        [] => list(kinds.as_deref(), labels),
        [source, dir] => {
            let share = kinds.as_deref().is_some_and(|kind| SHARES.contains(&kind));
            if share || (kinds.is_none() && is_share(source.as_bytes())) {
                return Err(serve(SMB, source, dir, &options, access));
            }
            let export = kinds.as_deref().is_some_and(|kind| EXPORTS.contains(&kind));
            if export || (kinds.is_none() && is_export(source.as_bytes())) {
                return Err(serve(NFS, source, dir, &options, access));
            }
            mount_disk(source, dir, kinds.as_deref(), &options, access)
        }
        [_] => Err(usage("give a source and a folder: Dukovo has no fstab to look one up".into())),
        _ => Err(usage("too many operands".into())),
    }
}

/// Lists the mounts, of the kinds `kinds` names if it names some: a
/// share's as `smb`'s, `cifs`'s or `fuse`'s, and an NFS folder's as
/// `nfs`'s, `nfs4`'s or `fuse`'s.
fn list(kinds: Option<&str>, labels: bool) -> Result<i32, Fail> {
    let mounts = proc::mounts().map_err(|e| util::failed(&"/processes/mounts", e))?;
    let wanted: Option<Vec<&str>> = kinds.map(|kinds| kinds.split(',').collect());
    let mut out = io::stdout().lock();
    for m in &mounts {
        if wanted.as_ref().is_some_and(|wanted| !wanted.iter().any(|kind| is_kind(m, kind))) {
            continue;
        }
        let access = if m.read_only { "ro" } else { "rw" };
        let mut line =
            format!("{} on {} type {} ({access})", m.source, df::mount_path(m).display(), m.kind);
        if let Some(label) = m.label.as_ref().filter(|_| labels) {
            line = format!("{line} [{label}]");
        }
        if writeln!(out, "{line}").is_err() {
            break;
        }
    }
    Ok(0)
}

/// Whether mount `m` is of the kind `kind` names.
fn is_kind(m: &Mount, kind: &str) -> bool {
    let fuse = m.kind == "fuse";
    m.kind == kind
        || (SHARES.contains(&kind) && fuse && is_share(m.source.as_bytes()))
        || (EXPORTS.contains(&kind) && fuse && is_export(m.source.as_bytes()))
}

/// Mounts a share or an NFS folder: `program`, smb(1) or nfs(1), does,
/// with these options, in this process's place. Returns only if it
/// cannot be run.
fn serve(
    program: &str,
    source: &OsString,
    dir: &OsString,
    options: &[String],
    access: Option<&str>,
) -> Fail {
    let mut command = Command::new(program);
    command.arg("mount");
    for options in options {
        command.args(["-o", options]);
    }
    if let Some(access) = access {
        command.args(["-o", access]);
    }
    command.arg(source).arg(dir);
    let err = command.exec();
    util::failed(&program, err)
}

/// Mounts the disk `source`, a device or its name under /devices, at
/// `dir`: as the kind `kind` names, or else as whichever it holds.
fn mount_disk(
    source: &OsString,
    dir: &OsString,
    kind: Option<&str>,
    options: &[String],
    access: Option<&str>,
) -> Result<i32, Fail> {
    let mut flags = MountFlags::NONE;
    let mut read_only = None;
    for option in options.iter().flat_map(|options| options.split(',')) {
        match option {
            "ro" => read_only = Some(true),
            "rw" => read_only = Some(false),
            "compress" | "compress=zstd" => flags = flags.union(MountFlags::COMPRESS),
            "defaults" | "" => {}
            other => return Err(usage(format!("{other}: not an option mount knows for a disk"))),
        }
    }
    if let Some(access) = access {
        read_only = Some(access == "ro");
    }
    let kinds: Vec<&[u8]> = match kind {
        None => KINDS.iter().map(|(kind, _)| *kind).collect(),
        Some(kind) => match KINDS.iter().find(|(name, _)| name == &kind.as_bytes()) {
            Some((kind, _)) => vec![*kind],
            None => {
                return Err(util::fail(format!("{kind}: not a kind of filesystem Dukovo mounts")));
            }
        },
    };
    let device = device_path(source);
    let (file, writable) =
        open(&device, read_only == Some(true)).map_err(|e| util::failed(&device.display(), e))?;
    let dir = std::fs::canonicalize(dir).map_err(|e| util::failed(&Path::new(dir).display(), e))?;
    let path = dir.as_os_str().as_bytes().strip_prefix(b"/").unwrap_or_default().to_vec();
    if !writable {
        if read_only == Some(false) {
            return Err(util::fail(format!(
                "{} cannot be written: mount it only to read (-r)",
                device.display()
            )));
        }
        flags = flags.union(MountFlags::READ_ONLY);
    } else if read_only == Some(true) {
        flags = flags.union(MountFlags::READ_ONLY);
    }
    let slot = sys::slot_of(file.as_raw_fd());
    let mut last = None;
    for kind in kinds {
        match mount_as(&path, kind, slot, flags, read_only.is_none()) {
            Ok(mounted) => {
                if mounted.contains(MountFlags::READ_ONLY) && !flags.contains(MountFlags::READ_ONLY)
                {
                    eprintln!(
                        "mount: {}: {} is not written here: mounted read-only",
                        dir.display(),
                        device.display()
                    );
                }
                return Ok(0);
            }
            Err(status) if FileError::from_status(status) == Some(FileError::UnknownFilesystem) => {
                last.get_or_insert(status);
            }
            Err(status) => {
                last = Some(status);
                break;
            }
        }
    }
    let status = last.unwrap_or(FileError::UnknownFilesystem.status(0));
    let why = match FileError::from_status(status) {
        Some(FileError::UnknownFilesystem) => {
            format!("{} holds no filesystem Dukovo mounts", device.display())
        }
        Some(FileError::InUse) => format!("{} is mounted already, or in use", device.display()),
        _ => util::strip_os_error(&sys::io_error(status)),
    };
    Err(util::fail(format!("cannot mount at {}: {why}", dir.display())))
}

/// The device `source` names: its path, or its name under /devices.
fn device_path(source: &OsString) -> PathBuf {
    let path = Path::new(source);
    if path.components().count() == 1 && !path.is_absolute() {
        Path::new("/devices").join(path)
    } else {
        path.to_path_buf()
    }
}

/// Opens the device to write, unless only to read, or when it refuses
/// writes: the file, and whether it may be written.
fn open(device: &Path, read_only: bool) -> io::Result<(File, bool)> {
    if read_only {
        return Ok((File::open(device)?, false));
    }
    match OpenOptions::new().read(true).write(true).open(device) {
        Ok(file) => Ok((file, true)),
        Err(err) if err.raw_os_error() == Some(libc::EROFS) => Ok((File::open(device)?, false)),
        Err(err) => Err(err),
    }
}

/// Mounts the device in `slot` at `path` as `kind`; one that the kernel
/// writes not, as for an NTFS, is mounted read-only if `may_read_only`,
/// as Linux's mount does. The flags it was mounted with.
fn mount_as(
    path: &[u8],
    kind: &[u8],
    slot: u32,
    flags: MountFlags,
    may_read_only: bool,
) -> Result<MountFlags, Status> {
    match directory::mount(&SYS, startup::SLOT_ROOT, path, kind, slot, flags) {
        Err(status)
            if may_read_only
                && !flags.contains(MountFlags::READ_ONLY)
                && (FileError::from_status(status) == Some(FileError::ReadOnly)
                    || Kernel::from_status(status) == Some(Kernel::InvalidArgument)) =>
        {
            let flags = flags.union(MountFlags::READ_ONLY);
            directory::mount(&SYS, startup::SLOT_ROOT, path, kind, slot, flags).map(|()| flags)
        }
        done => done.map(|()| flags),
    }
}

pub fn umount(args: &mut Args) -> Result<i32, Fail> {
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) => operands.push(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    if operands.is_empty() {
        return Err(usage("give a folder, or the source mounted there".into()));
    }
    let mounts = proc::mounts().map_err(|e| util::failed(&"/processes/mounts", e))?;
    let mut status = 0;
    for operand in &operands {
        if let Err(why) = unmount(&mounts, operand) {
            eprintln!("umount: {}: {why}", Path::new(operand).display());
            status = 1;
        }
    }
    Ok(status)
}

/// Whether mount `m` is from the source `what`: as the mounts list names
/// it, or a disk by its path or its name under /devices.
fn is_from(m: &Mount, what: &OsString) -> bool {
    m.source.as_bytes() == what.as_bytes()
        || (!is_share(what.as_bytes())
            && !is_export(what.as_bytes())
            && device_path(&m.source.clone().into()) == device_path(what))
}

/// Unmounts the filesystem at the folder `what`, or the last one mounted
/// from the source `what`.
fn unmount(mounts: &[Mount], what: &OsString) -> Result<(), String> {
    let path = match std::fs::canonicalize(what) {
        Ok(path) if mounts.iter().any(|m| df::mount_path(m) == path) => path,
        found => match mounts.iter().rev().find(|m| is_from(m, what)) {
            Some(m) => df::mount_path(m),
            None => {
                return Err(match found {
                    Ok(_) => "not a mount".to_string(),
                    Err(err) => util::strip_os_error(&err),
                });
            }
        },
    };
    let relative = path.as_os_str().as_bytes().strip_prefix(b"/").unwrap_or_default();
    directory::unmount(&SYS, startup::SLOT_ROOT, relative).map_err(|status| {
        match (FileError::from_status(status), Kernel::from_status(status)) {
            (Some(FileError::InUse), _) => {
                "busy: files are open on it, or something is mounted on it".to_string()
            }
            (_, Some(Kernel::MissingRights | Kernel::NotSupported)) => {
                "unmounting needs an administrator's session (elevate)".to_string()
            }
            _ => util::strip_os_error(&sys::io_error(status)),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(source: &str, path: &str, kind: &str) -> Mount {
        Mount {
            source: source.into(),
            path: path.into(),
            kind: kind.into(),
            read_only: false,
            label: None,
        }
    }

    #[test]
    fn shares_are_named_by_their_source_or_kind() {
        assert!(
            is_share(b"//nas/media") && is_share(b"\\\\nas\\media") && is_share(b"smb://nas/m")
        );
        assert!(!is_share(b"/devices/vdb1") && !is_share(b"vdb1"));
        let share = mount("//nas/media", "home/ann/media", "fuse");
        assert!(is_kind(&share, "cifs") && is_kind(&share, "smb") && is_kind(&share, "fuse"));
        assert!(!is_kind(&share, "btrfs"));
        let disk = mount("vdb1", "volumes/data", "btrfs");
        assert!(is_kind(&disk, "btrfs") && !is_kind(&disk, "cifs"));
        assert!(!is_kind(&mount("fuse-test", "tmp/fuse-test", "fuse"), "smb"));
    }

    #[test]
    fn nfs_folders_are_named_by_their_source_or_kind() {
        for source in ["nas:/srv/music", "10.0.2.2:/", "[fe80::1]:/x", "nfs://nas/srv"] {
            assert!(is_export(source.as_bytes()), "{source}");
        }
        for source in ["/devices/vdb1", "vdb1", ":/x", "a/b:/c", "//nas/media", "nas:x"] {
            assert!(!is_export(source.as_bytes()), "{source}");
        }
        let folder = mount("nas:/srv/music", "home/ann/music", "fuse");
        assert!(is_kind(&folder, "nfs") && is_kind(&folder, "nfs4") && is_kind(&folder, "fuse"));
        assert!(!is_kind(&folder, "smb"));
        assert!(!is_kind(&mount("//nas/media", "m", "fuse"), "nfs"));
    }

    #[test]
    fn devices_by_name_or_path() {
        assert_eq!(device_path(&"vdb1".into()), Path::new("/devices/vdb1"));
        assert_eq!(device_path(&"/devices/vdb1".into()), Path::new("/devices/vdb1"));
        assert_eq!(device_path(&"./disk.img".into()), Path::new("./disk.img"));
    }

    #[test]
    fn a_disk_is_unmounted_by_its_name_or_path() {
        let disk = mount("vdb1", "volumes/data", "btrfs");
        assert!(is_from(&disk, &"vdb1".into()) && is_from(&disk, &"/devices/vdb1".into()));
        assert!(!is_from(&disk, &"vdb2".into()) && !is_from(&disk, &"/devices/vdb".into()));
        let share = mount("//nas/media", "home/ann/media", "fuse");
        assert!(is_from(&share, &"//nas/media".into()) && !is_from(&share, &"//nas".into()));
    }
}
