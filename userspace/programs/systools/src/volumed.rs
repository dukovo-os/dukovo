// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! volumed(8): mounts each volume plugged in after boot at
//! `/volumes/<label>`, as udisks does for a desktop's removable media,
//! and unmounts the volumes of a disk that goes (ADR 0163); eject(1)
//! unmounts one before its disk is pulled out.
//!
//! Spark mounts the disks there at boot, each at `/volumes/<name>`;
//! volumed takes those it finds at its start as done, and looks at
//! `/kernel/block` every second for disks and partitions that come or go.
//! A partition that comes on a disk that was there already was made by
//! partitioning it (rdisk(8)), not plugged in, and is left unmounted for
//! whoever made it. As disks come and go, volumed keeps the links of
//! `/devices/disk` (ADR 0303).
//!
//! A drive whose medium comes and goes, such as a card reader's slot,
//! stays while it is empty (ADR 0430). The kernel counts each medium that
//! goes in or comes out, in `/kernel/block/<drive>/media_changes`; when
//! the count moves, what was on the drive is gone and what is on it now
//! came, though the names be the same: the old medium's volumes are
//! unmounted, and the new one's mounted.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::Duration;

use radix_abi::directory::{self, MountFlags};
use radix_abi::errors::{File as FileError, Kernel};
use radix_abi::{Status, startup};

use crate::args::{Arg, Args};
use crate::probe::{self, BLOCKS};
use crate::proc::{self, Mount};
use crate::sys::{self, SYS};
use crate::util::{self, Fail};
use crate::{df, links};

/// How often `/kernel/block` is looked at.
const PERIOD: Duration = Duration::from_secs(1);

/// The type of an EFI system partition, as `/kernel/block` gives it: in
/// a GPT, and in an MBR. Spark leaves these alone, and so does volumed.
const EFI_SYSTEM: [&str; 2] = ["c12a7328-f81f-11d2-ba4b-00a0c93ec93b", "0xef"];

/// The type of an install stick's DUKOVO_LIVE partition, "Dukovo live"
/// (ADR 0308): the signed release the installer installs, mounted
/// read-only, by Spark and by volumed alike.
const DUKOVO_LIVE: &str = "5894c662-af90-4b47-baaf-2e3396443b25";

/// The filesystems tried, in Spark's order: FAT, exFAT and NTFS before
/// the others, whose superblocks their `mkfs` can leave behind, and discs
/// last, UDF before the ISO 9660 a DVD has as a bridge to it.
pub const KINDS: [(&[u8], &str); 7] = [
    (b"fat", "FAT"),
    (b"exfat", "exFAT"),
    (b"ntfs", "NTFS"),
    (b"btrfs", "Btrfs"),
    (b"ext4", "ext4"),
    (b"udf", "UDF"),
    (b"iso9660", "ISO 9660"),
];

/// What `/kernel/block` has.
#[derive(Default)]
struct Present {
    /// Its disks and partitions.
    names: BTreeSet<String>,
    /// For each drive whose medium comes and goes, how many times one went
    /// in or came out.
    changes: BTreeMap<String, u64>,
}

/// The disks and partitions there are now.
fn names() -> BTreeSet<String> {
    std::fs::read_dir(BLOCKS).map_or_else(
        |_| BTreeSet::new(),
        |entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect(),
    )
}

/// How many times the medium of each of `drives` went in or came out.
fn changes<'a>(drives: impl Iterator<Item = &'a String>) -> BTreeMap<String, u64> {
    drives
        .filter_map(|drive| {
            let count = probe::block_text(drive, "media_changes")?.parse().ok()?;
            Some((drive.clone(), count))
        })
        .collect()
}

/// What `/kernel/block` has now. `drives` says of each name seen whether
/// it is a drive whose medium comes and goes, and learns the new ones'.
/// The drives' counts are read before the names and after, and all again
/// until they agree, so that the names are those of the media the counts
/// say: the kernel changes a drive's medium and its count at once.
fn present(drives: &mut BTreeMap<String, bool>) -> Present {
    let is_drive = |name: &String| probe::block_text(name, "removable").as_deref() == Some("1");
    let mut now = Present::default();
    for _ in 0..5 {
        let listed = |drives: &BTreeMap<String, bool>| {
            changes(drives.iter().filter(|(_, drive)| **drive).map(|(name, _)| name))
        };
        let before = listed(drives);
        now.names = names();
        let new: Vec<String> =
            now.names.iter().filter(|n| !drives.contains_key(*n)).cloned().collect();
        for name in new {
            let drive = is_drive(&name);
            drives.insert(name, drive);
        }
        drives.retain(|name, _| now.names.contains(name));
        now.changes = listed(drives);
        if now.changes.iter().all(|(drive, count)| before.get(drive).is_none_or(|was| was == count))
        {
            break;
        }
    }
    now
}

/// Whether `name` is the disk `disk` or one of its partitions: `sdb1` of
/// `sdb`, `mmcblk0p1` of `mmcblk0`; `cd10` is not `cd1`'s.
fn of_disk(disk: &str, name: &str) -> bool {
    let Some(rest) = name.strip_prefix(disk) else { return false };
    let digit = disk.ends_with(|c: char| c.is_ascii_digit());
    let rest = if digit && !rest.is_empty() { rest.strip_prefix('p') } else { Some(rest) };
    rest.is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether `name` is a volume: a partition, or a disk with none, that is
/// no EFI system partition. An empty drive, which has no sectors, is none.
fn is_volume(name: &str, present: &BTreeSet<String>) -> bool {
    let partitioned = probe::block_text(name, "partition").is_none()
        && present.iter().any(|other| other != name && probe::disk_of(other) == name);
    let efi = probe::block_text(name, "parttype").is_some_and(|kind| EFI_SYSTEM.contains(&&*kind));
    let empty = probe::block_text(name, "size").as_deref() == Some("0");
    !partitioned && !efi && !empty
}

/// The name a volume is mounted at: its filesystem's label, made a single
/// file name, or the device's name for a volume with none; with the
/// device's name after a dash when another volume has the label.
pub fn place_for(label: Option<&str>, name: &str, taken: impl Fn(&str) -> bool) -> String {
    let label: String = label
        .unwrap_or_default()
        .trim()
        .chars()
        .map(|c| if c == '/' || c.is_control() { '_' } else { c })
        .collect();
    let label = match label.as_str() {
        "" | "." | ".." => return name.to_string(),
        _ => label,
    };
    if taken(&label) { format!("{label}-{name}") } else { label }
}

/// Opens `/devices/<name>` to write, or to read when it refuses writes or
/// is an install stick's DUKOVO_LIVE.
fn open_device(name: &str) -> io::Result<(File, MountFlags)> {
    let path = format!("/devices/{name}");
    if probe::block_text(name, "parttype")
        .is_some_and(|kind| kind.eq_ignore_ascii_case(DUKOVO_LIVE))
    {
        return Ok((File::open(&path)?, MountFlags::READ_ONLY));
    }
    match OpenOptions::new().read(true).write(true).open(&path) {
        Ok(file) => Ok((file, MountFlags::NONE)),
        Err(err) if err.raw_os_error() == Some(libc::EROFS) => {
            Ok((File::open(&path)?, MountFlags::READ_ONLY))
        }
        Err(err) => Err(err),
    }
}

/// The directory to mount beneath at the absolute `path`, as a slot, and
/// `path` beneath it: a place Spark handed this program as a descriptor
/// from 3 on, as its service file's `files.mount` asks, which
/// `MOUNT_PLACES` names in order (ADR 0217), since nothing is mounted
/// through a view; or else the root, as for eject(1) in root's shell.
fn mount_base(path: &str) -> (u32, String) {
    let places = std::env::var("MOUNT_PLACES").unwrap_or_default();
    for (fd, place) in (3..).zip(places.split(':').filter(|p| !p.is_empty())) {
        if let Some(rest) = path.strip_prefix(place).and_then(|r| r.strip_prefix('/')) {
            return (sys::slot_of(fd), rest.to_string());
        }
    }
    (startup::SLOT_ROOT, path.trim_start_matches('/').to_string())
}

/// Mounts `device` at `/volumes/<at>` as a filesystem of `kind`, read-only
/// if it refuses writes or Radix does not write it, as Spark does.
fn mount_as(at: &str, kind: &[u8], device: &File, flags: MountFlags) -> Result<MountFlags, Status> {
    let (base, path) = mount_base(&format!("/volumes/{at}"));
    let source = sys::slot_of(device.as_raw_fd());
    match directory::mount(&SYS, base, path.as_bytes(), kind, source, flags) {
        Err(status)
            if !flags.contains(MountFlags::READ_ONLY)
                && (FileError::from_status(status) == Some(FileError::ReadOnly)
                    || Kernel::from_status(status) == Some(Kernel::InvalidArgument)) =>
        {
            let flags = MountFlags::READ_ONLY;
            directory::mount(&SYS, base, path.as_bytes(), kind, source, flags).map(|()| flags)
        }
        done => done.map(|()| flags),
    }
}

/// Mounts the volume `name`, which just came: a line of what it did. A
/// locked BitLocker volume is left for unlock(8), whose unlocked device
/// comes next, and is mounted then (ADR 0240).
fn mount(name: &str) -> String {
    let found = probe::reader(name).and_then(|mut read| probe::filesystem(&mut read));
    if found.as_ref().is_some_and(|found| found.kind == "BitLocker") {
        return format!("{name} is a locked BitLocker volume; `unlock {name}` unlocks it");
    }
    let label = found.and_then(|found| found.label);
    let at = place_for(label.as_deref(), name, |at| Path::new("/volumes").join(at).exists());
    let (device, flags) = match open_device(name) {
        Ok(opened) => opened,
        Err(err) => return format!("cannot open /devices/{name}: {}", util::strip_os_error(&err)),
    };
    if let Err(err) = std::fs::create_dir(format!("/volumes/{at}")) {
        return format!("cannot make /volumes/{at}: {}", util::strip_os_error(&err));
    }
    let mut last = None;
    for (kind, said) in KINDS {
        match mount_as(&at, kind, &device, flags) {
            Ok(flags) => {
                let ro = if flags.contains(MountFlags::READ_ONLY) { ", read-only" } else { "" };
                return format!("mounted the {said} filesystem of {name} at /volumes/{at}{ro}");
            }
            Err(status) if FileError::from_status(status) == Some(FileError::UnknownFilesystem) => {
            }
            Err(status) => {
                last = Some(status);
                break;
            }
        }
    }
    let _ = std::fs::remove_dir(format!("/volumes/{at}"));
    match last {
        None => format!(
            "{name} holds no FAT, exFAT, NTFS, Btrfs, ext4, UDF or ISO 9660 filesystem; left alone"
        ),
        Some(status) if FileError::from_status(status) == Some(FileError::InUse) => {
            format!("{name} is in use already; left alone")
        }
        Some(status) => {
            format!("cannot mount {name}: {}", util::strip_os_error(&sys::io_error(status)))
        }
    }
}

/// The mounts at `/volumes/<one name>`.
pub fn volume_mounts() -> io::Result<Vec<Mount>> {
    Ok(proc::mounts()?
        .into_iter()
        .filter(|m| {
            let path = df::mount_path(m);
            path.parent() == Some(Path::new("/volumes"))
        })
        .collect())
}

/// Unmounts the volume at `path` and removes its directory.
pub fn unmount(path: &Path) -> Result<(), String> {
    let (base, relative) = mount_base(&path.to_string_lossy());
    directory::unmount(&SYS, base, relative.as_bytes())
        .map_err(|status| util::strip_os_error(&sys::io_error(status)))?;
    std::fs::remove_dir(path).map_err(|err| util::strip_os_error(&err))
}

/// Mounts the volume `mount` at `/volumes` again, as `flags` ask (ADR
/// 0238): unmounted, then mounted from its device with its filesystem; one
/// that refuses writes is mounted read-only, as at its first mount. A
/// mount that fails is tried again as it was.
pub fn remount(mount: &Mount, flags: MountFlags) -> Result<MountFlags, String> {
    let path = df::mount_path(mount);
    let at = path.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("not a volume")?;
    let (device, device_flags) =
        open_device(&mount.source).map_err(|err| util::strip_os_error(&err))?;
    let flags = if device_flags.contains(MountFlags::READ_ONLY) {
        flags.union(MountFlags::READ_ONLY)
    } else {
        flags
    };
    unmount(&path)?;
    std::fs::create_dir(&path).map_err(|err| util::strip_os_error(&err))?;
    let kind = mount.kind.as_bytes();
    match mount_as(&at, kind, &device, flags) {
        Ok(flags) => Ok(flags),
        Err(status) => {
            let was = if mount.read_only { MountFlags::READ_ONLY } else { MountFlags::NONE };
            let _ = mount_as(&at, kind, &device, was);
            Err(util::strip_os_error(&sys::io_error(status)))
        }
    }
}

/// Unmounts what is mounted at `/volumes` from the disks and partitions
/// in `gone`: lines of what it did.
fn unmount_gone(gone: &BTreeSet<String>) -> Vec<String> {
    let Ok(mounts) = volume_mounts() else { return Vec::new() };
    let mut said = Vec::new();
    for mount in mounts.iter().filter(|m| gone.contains(&m.source)) {
        let path = df::mount_path(mount);
        said.push(match unmount(&path) {
            Ok(()) => format!("{} is gone; unmounted {}", mount.source, path.display()),
            Err(why) => {
                format!("{} is gone; cannot unmount {}: {why}", mount.source, path.display())
            }
        });
    }
    said
}

/// Whether `name`, which just came, is a partition of a disk that was
/// there before, in `known`: one made by partitioning the disk again.
fn repartitioned(name: &str, known: &BTreeSet<String>) -> bool {
    let disk = probe::disk_of(name);
    disk != name && known.contains(disk)
}

/// The drives of `now` whose medium went in or came out since `known`.
fn swapped(known: &Present, now: &Present) -> BTreeSet<String> {
    let moved =
        |(drive, count): (&String, &u64)| known.changes.get(drive).is_some_and(|was| was != count);
    now.changes.iter().filter(|&entry| moved(entry)).map(|(drive, _)| drive.clone()).collect()
}

/// What volumed says of `drive`, whose medium went in or came out.
fn swap_note(drive: &str) -> String {
    let empty = probe::block_text(drive, "size").as_deref() == Some("0");
    disc_note(drive, empty, probe::block_text(drive, "disc").as_deref()).unwrap_or_else(|| {
        if empty {
            format!("{drive} has no medium in it now")
        } else {
            format!("a medium went in {drive}")
        }
    })
}

/// What volumed says of the optical drive `drive`, `empty` or not, whose
/// disc went in or came out, as its `/kernel/block/<drive>/disc` says
/// what is in it (ADR 0431): `none`, a disc's kind and what it holds
/// (`dvd-rom data`, `cd-rom audio`, `dvd+r blank`), or `unreadable`.
/// `None` for another drive.
fn disc_note(drive: &str, empty: bool, disc: Option<&str>) -> Option<String> {
    let disc = disc?.trim();
    let (kind, holds) = disc.split_once(' ').unwrap_or((disc, ""));
    Some(match (kind, holds) {
        ("none", _) => format!("{drive} has no disc in it now"),
        ("unreadable", _) => format!("a disc went in {drive}, but it cannot be read"),
        (_, "audio") => {
            format!("a music CD went in {drive}; its tracks are not files, and it is not mounted")
        }
        (kind, "blank") => format!("a blank disc ({kind}) went in {drive}; nothing to mount"),
        (kind, _) if !empty => format!("a disc ({kind}) went in {drive}"),
        // A disc of data the kernel could not set up: its log says why.
        (kind, _) => format!("a disc ({kind}) went in {drive}, but it is not used"),
    })
}

/// Watches `/kernel/block` for good.
fn watch() -> Result<i32, Fail> {
    let mut drives = BTreeMap::new();
    let mut known = present(&mut drives);
    let mut out = io::stdout().lock();
    if let Err(err) = links::refresh() {
        writeln!(out, "cannot make the links of /devices/disk: {}", util::strip_os_error(&err))?;
    }
    writeln!(out, "watching for volumes; {} disks and partitions now", known.names.len())?;
    loop {
        std::thread::sleep(PERIOD);
        let now = present(&mut drives);
        // What was on a drive whose medium changed is gone, and what is
        // on it now came, though the names be the same.
        let swapped = swapped(&known, &now);
        let on_swapped = |name: &String| swapped.iter().any(|drive| of_disk(drive, name));
        let gone: BTreeSet<String> = known
            .names
            .iter()
            .filter(|name| !now.names.contains(*name) || on_swapped(name))
            .cloned()
            .collect();
        for line in unmount_gone(&gone) {
            writeln!(out, "{line}")?;
        }
        for drive in &swapped {
            writeln!(out, "{}", swap_note(drive))?;
        }
        let came = now.names.iter().filter(|name| !known.names.contains(*name) || on_swapped(name));
        for name in came.filter(|name| is_volume(name, &now.names)) {
            if !on_swapped(name) && repartitioned(name, &known.names) {
                let disk = probe::disk_of(name);
                writeln!(
                    out,
                    "{name} is new on {disk}, which was partitioned again; left unmounted"
                )?;
                continue;
            }
            writeln!(out, "{}", mount(name))?;
        }
        if now.names != known.names || !swapped.is_empty() {
            links::refresh_quietly();
        }
        known = now;
    }
}

pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut once = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(long) if long == "list" => once = true,
            other => return Err(args.unknown(&other)),
        }
    }
    if !once {
        return watch();
    }
    let mounts = volume_mounts().map_err(|e| util::failed(&"/processes/mounts", e))?;
    let mut out = io::stdout().lock();
    for mount in &mounts {
        let ro = if mount.read_only { " (read-only)" } else { "" };
        let path = df::mount_path(mount);
        writeln!(out, "{}  /devices/{}  {}{ro}", path.display(), mount.source, mount.kind)?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_come_from_labels() {
        let free = |_: &str| false;
        assert_eq!(place_for(Some("DUKOVO"), "sdb1", free), "DUKOVO");
        assert_eq!(place_for(Some("  My Stick "), "sdb1", free), "My Stick");
        assert_eq!(place_for(Some("a/b"), "sdb1", free), "a_b");
        assert_eq!(place_for(None, "sdb1", free), "sdb1");
        assert_eq!(place_for(Some(".."), "sdb1", free), "sdb1");
        assert_eq!(place_for(Some("DUKOVO"), "sdc", |at| at == "DUKOVO"), "DUKOVO-sdc");
    }

    #[test]
    fn a_disk_has_its_partitions() {
        assert!(of_disk("sdb", "sdb"));
        assert!(of_disk("sdb", "sdb1"));
        assert!(of_disk("mmcblk0", "mmcblk0p2"));
        assert!(!of_disk("sdb", "sdc1"));
        assert!(!of_disk("sdb", "sdb1-unlocked"));
        assert!(of_disk("cd1", "cd1"));
        assert!(!of_disk("cd1", "cd10"));
        assert!(!of_disk("nvme0n1", "nvme0n10"));
        assert!(of_disk("nvme0n1", "nvme0n1p10"));
    }

    #[test]
    fn only_drives_whose_count_moved_are_swapped() {
        let counts = |pairs: &[(&str, u64)]| Present {
            names: BTreeSet::new(),
            changes: pairs.iter().map(|&(d, c)| (d.to_string(), c)).collect(),
        };
        let known = counts(&[("sdb", 0), ("sdc", 2)]);
        let now = counts(&[("sdb", 1), ("sdc", 2), ("sdd", 0)]);
        assert_eq!(swapped(&known, &now), BTreeSet::from(["sdb".to_string()]));
    }

    #[test]
    fn a_drive_says_what_disc_went_in() {
        let note = |empty, disc| disc_note("cd0", empty, disc);
        assert_eq!(note(true, None), None);
        assert_eq!(note(true, Some("none\n")).unwrap(), "cd0 has no disc in it now");
        assert_eq!(note(false, Some("dvd-rom data")).unwrap(), "a disc (dvd-rom) went in cd0");
        assert!(note(true, Some("cd-rom audio")).unwrap().starts_with("a music CD went in cd0"));
        assert_eq!(
            note(true, Some("dvd+r blank")).unwrap(),
            "a blank disc (dvd+r) went in cd0; nothing to mount"
        );
        assert!(note(true, Some("unreadable")).unwrap().contains("cannot be read"));
        assert!(note(true, Some("bd-rom data")).unwrap().ends_with("but it is not used"));
    }

    #[test]
    fn partitions_new_on_a_disk_there_before_are_repartitioned() {
        let known: BTreeSet<String> = ["vda", "vda1", "nvme0n1"].map(String::from).into();
        assert!(repartitioned("vda2", &known));
        assert!(repartitioned("nvme0n1p1", &known));
        assert!(!repartitioned("sdb1", &known));
        assert!(!repartitioned("sdb", &known));
    }
}
