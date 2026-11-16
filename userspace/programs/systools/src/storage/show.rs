// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `storage show`: the disks, the Btrfs volumes and the other filesystems
//! of the machine, joined from the files `lsblk`, `blkid`, `df`,
//! `diskhealth` and `scrub` read.
//!
//! Anyone may run it. It reads `/kernel/block` and `/processes/mounts`,
//! what blkid finds on each device, statvfs of each mount, the folder of
//! each Btrfs volume in `/kernel/fs/btrfs`, the live files of the
//! diskhealth service and the kept results of scrub, and none of them
//! needs more than the right to read. What it cannot read it leaves null,
//! and does not fail: a disk the service has not looked at has no health,
//! a volume never scrubbed has no scrub, and a device that may not be
//! opened shows the filesystem and the label its mount gives, and no ID.
//!
//! The JSON is what the web console's Storage page reads, so its fields
//! and their order are the interface: [`Report`] and the structs below it
//! are in the order the fields are written, and the golden file of this
//! module's tests names them all.

use std::fs;
use std::path::{Path, PathBuf};

use disk::table::Scheme;
use disk::types;
use serde::Serialize;

use super::{Group, SECTOR, layout, text_of};
use crate::df::{self, Space};
use crate::diskhealth::{self, Live, Verdict};
use crate::proc::{self, Mount};
use crate::scrub::{self, Kept};
use crate::util::{self, Fail, fail};
use crate::{blkid, probe};

/// Where the Btrfs volumes of the machine are in the kernel's files.
pub(crate) const BTRFS: &str = "/kernel/fs/btrfs";

/// The folders a volume holds when one of its mounts is at the path, in
/// the order [`Volume::holds`] gives them.
const HOLDS: [&str; 3] = ["/users", "/shared", "/applications"];

/// What GPT keeps after the last partition: the backup of the table, 33
/// sectors.
const GPT_END: u64 = 33 * SECTOR;

/// Where `storage show` reads, and how it asks devices, so that the tests
/// can read their own.
pub(super) struct Sources {
    /// The kernel's disks and partitions.
    blocks: PathBuf,
    /// The mounts.
    mounts: PathBuf,
    /// The kernel's folder of each Btrfs volume.
    btrfs: PathBuf,
    /// The diskhealth service's live file of each disk.
    health: PathBuf,
    /// The kept result of each filesystem's last scrub.
    scrubs: PathBuf,
    /// What blkid finds on a device: its tags.
    tags: fn(&str) -> Vec<(&'static str, String)>,
    /// What statvfs says of the filesystem at a path.
    space: fn(&Path) -> Option<Space>,
}

impl Sources {
    /// The places of a running machine.
    fn system() -> Sources {
        Sources {
            blocks: PathBuf::from(probe::BLOCKS),
            mounts: PathBuf::from(format!("{}/mounts", proc::PROC)),
            btrfs: PathBuf::from(BTRFS),
            health: PathBuf::from(diskhealth::LIVE),
            scrubs: PathBuf::from(scrub::KEPT),
            tags: blkid::tags,
            space: |path| df::statvfs(path.as_os_str()).ok(),
        }
    }
}

/// What `storage show --json` writes.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Report {
    /// The shape of this object: 1.
    format: u8,
    /// The disks, in the order of their names.
    disks: Vec<Disk>,
    /// The Btrfs volumes mounted, in the order of their IDs.
    volumes: Vec<Volume>,
    /// The mounted filesystems that are not Btrfs and are on a disk of
    /// the list above, in the order they were mounted.
    filesystems: Vec<Filesystem>,
}

/// One disk.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Disk {
    name: String,
    /// What the driver says of it, null where it says nothing.
    model: Option<String>,
    serial: Option<String>,
    /// Its size in bytes, which is 0 for a drive with no medium in it.
    size: u64,
    /// Whether its medium comes and goes, and whether it spins; null where
    /// the driver does not say.
    removable: Option<bool>,
    rotational: Option<bool>,
    /// How it is connected, where its name says: `nvme`, `virtio`,
    /// `sd-card`, `floppy` or `optical`. An `sd` disk may be on a SATA or
    /// a USB port, which the kernel does not say, so it is null.
    bus: Option<&'static str>,
    partitions: Vec<Partition>,
    /// The bytes from the end of the last partition to the end of the
    /// disk less the 33 sectors GPT keeps there: an estimate, which `grow`
    /// checks exactly. Null for a disk with no partition.
    free_after: Option<u64>,
    /// What the diskhealth service last said of it, null when it has not
    /// looked or the disk gives none.
    health: Option<Health>,
}

/// One partition.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Partition {
    name: String,
    /// Its number in the disk's table.
    number: u32,
    /// Where it begins on the disk, and its size, in bytes.
    start: u64,
    size: u64,
    /// Its type, as rdisk(8) names it (`efi`, `swap`, `linux`), or as the
    /// kernel gives it, a GUID or an MBR type byte, when rdisk has no name
    /// for it; null when the kernel gives none.
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Its name in a GPT, null where the table has none.
    gpt_name: Option<String>,
    /// What blkid finds on it (`btrfs`, `vfat`, `swap`), or, when it may
    /// not be opened, the kind of its mount; null for none.
    filesystem: Option<String>,
    label: Option<String>,
    uuid: Option<String>,
    /// The places it is mounted at, in the order they were mounted.
    mounts: Vec<String>,
}

/// What the diskhealth service last said of a disk.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Health {
    verdict: Verdict,
    /// When the service looked, in milliseconds since 1970.
    checked_ms: u64,
    hours: Option<u64>,
    temperature_c: Option<i64>,
    /// A SATA disk's counts: sectors moved to spares, waiting to be
    /// moved, and not readable offline.
    moved: Option<u64>,
    waiting: Option<u64>,
    offline: Option<u64>,
    /// An NVMe drive's media errors and its share of its rated endurance
    /// used, in percent.
    media_errors: Option<u64>,
    used_percent: Option<u8>,
    /// The early warning that stands, null when none does.
    rose: Option<Rose>,
}

/// An early warning of a disk, as the service keeps it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Rose {
    at_ms: u64,
    title: String,
}

/// One Btrfs volume.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Volume {
    uuid: String,
    /// Its label, null when it has none.
    label: Option<String>,
    /// Where it is mounted: under `/volumes` if it is, else the first
    /// place; null when none of the devices that hold it is mounted.
    mount: Option<String>,
    /// Bytes, as statvfs says of the mount; null when it says nothing.
    size: Option<u64>,
    used: Option<u64>,
    free: Option<u64>,
    /// The devices whose filesystem has this ID, as far as they can be
    /// read.
    devices: Vec<String>,
    /// How many disks the kernel says it has, how many of them are
    /// missing, and how many are behind (radix-sysfs(5)).
    disks: Option<u64>,
    missing: Option<u64>,
    behind: Option<u64>,
    /// How its block groups keep their copies, null when the kernel's
    /// file cannot be read.
    profiles: Option<Profiles>,
    /// Which of `/users`, `/shared` and `/applications` are mounted from
    /// it.
    holds: Vec<&'static str>,
    /// The result of its last scrub, null when none is kept.
    scrub: Option<Kept>,
}

/// The profiles of a volume's block groups: for each kind a list, which
/// has two while a balance converts it, and none for a kind with no
/// chunks.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
struct Profiles {
    data: Vec<String>,
    metadata: Vec<String>,
    system: Vec<String>,
}

/// A mounted filesystem that is not Btrfs.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Filesystem {
    mount: String,
    /// Its type, as the `filesystem` of a partition has it.
    #[serde(rename = "type")]
    kind: Option<String>,
    /// The disk or partition it is on.
    device: String,
    size: Option<u64>,
    used: Option<u64>,
    free: Option<u64>,
}

/// What blkid finds on a device, and its mount or the kernel's Btrfs
/// volumes say in its place.
struct Found {
    filesystem: Option<String>,
    label: Option<String>,
    uuid: Option<String>,
    gpt_name: Option<String>,
}

/// The value of the tag `name` in `tags`, if it has one that is not
/// empty.
fn tag(tags: &[(&'static str, String)], name: &str) -> Option<String> {
    tags.iter()
        .find(|(tag, _)| *tag == name)
        .map(|(_, value)| value.clone())
        .filter(|v| !v.is_empty())
}

/// A Btrfs volume as the kernel tells it to anyone: its ID, its label, and
/// the disks and partitions it is on, which a kernel before the file
/// `devices` does not say.
struct Known {
    uuid: String,
    label: Option<String>,
    devices: Option<Vec<String>>,
}

/// What the kernel says of each Btrfs volume mounted, in the order of
/// their IDs.
fn known_volumes(sources: &Sources) -> Vec<Known> {
    volume_ids(sources)
        .into_iter()
        .map(|uuid| {
            let dir = sources.btrfs.join(&uuid);
            let read = |file: &str| fs::read_to_string(dir.join(file)).ok();
            Known {
                label: read("label").map(|text| text.trim().to_string()).filter(|l| !l.is_empty()),
                devices: read("devices").map(|text| {
                    text.lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(String::from)
                        .collect()
                }),
                uuid,
            }
        })
        .collect()
}

/// What is found on the device `name`: blkid's tags, or, for what they
/// do not say, as to one who may not open the device, what the mount from
/// it says and what the kernel says of the Btrfs volume it is on.
fn found(sources: &Sources, mounts: &[Mount], known: &[Known], name: &str) -> Found {
    let tags = (sources.tags)(name);
    let mount = mounts.iter().find(|mount| mount.source == name);
    let volume =
        known.iter().find(|volume| volume.devices.iter().flatten().any(|device| device == name));
    Found {
        filesystem: tag(&tags, "TYPE")
            .or_else(|| mount.map(|mount| mount.kind.clone()))
            .or_else(|| volume.map(|_| "btrfs".to_string())),
        label: tag(&tags, "LABEL")
            .or_else(|| mount.and_then(|mount| mount.label.clone()).filter(|l| !l.is_empty()))
            .or_else(|| volume.and_then(|volume| volume.label.clone())),
        uuid: tag(&tags, "UUID").or_else(|| volume.map(|volume| volume.uuid.clone())),
        gpt_name: tag(&tags, "PARTLABEL"),
    }
}

/// The type of a partition from the kernel's `parttype`: rdisk's name for
/// it, else the GUID or the type byte as it is.
fn type_name(text: &str) -> String {
    let scheme = if text.contains('-') { Scheme::Gpt } else { Scheme::Mbr };
    types::parse(text, scheme)
        .and_then(types::Kind::alias)
        .map_or_else(|| text.to_string(), str::to_string)
}

/// How a disk is connected, where its name says.
fn bus_of(name: &str) -> Option<&'static str> {
    [("nvme", "nvme"), ("vd", "virtio"), ("mmcblk", "sd-card"), ("fd", "floppy"), ("cd", "optical")]
        .into_iter()
        .find(|(prefix, _)| name.starts_with(prefix))
        .map(|(_, bus)| bus)
}

/// `1` and `0` as the kernel writes a flag.
fn flag_of(root: &Path, name: &str, what: &str) -> Option<bool> {
    match text_of(root, name, what)?.as_str() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

/// What a driver says in the file `what` of a disk, null for none.
fn said(root: &Path, name: &str, what: &str) -> Option<String> {
    text_of(root, name, what).filter(|text| !text.is_empty())
}

/// What the service last said of the disk `name`.
fn health_of(sources: &Sources, name: &str) -> Option<Health> {
    let text = fs::read_to_string(sources.health.join(format!("{name}.json"))).ok()?;
    let live: Live = serde_json::from_str(&text).ok()?;
    let Live { summary, checked_ms, rose, .. } = live;
    Some(Health {
        verdict: summary.verdict,
        checked_ms,
        hours: summary.hours,
        temperature_c: summary.temperature_c,
        moved: summary.moved,
        waiting: summary.waiting,
        offline: summary.offline,
        media_errors: summary.media_errors,
        used_percent: summary.used_percent,
        rose: rose.map(|rose| Rose { at_ms: rose.at_ms, title: rose.title }),
    })
}

/// The places a device is mounted at.
fn places(mounts: &[Mount], name: &str) -> Vec<String> {
    mounts
        .iter()
        .filter(|mount| mount.source == name)
        .map(|mount| df::mount_path(mount).to_string_lossy().into_owned())
        .collect()
}

/// A disk, its partitions and what is found on each.
fn disk_of(sources: &Sources, mounts: &[Mount], known: &[Known], group: &Group) -> Disk {
    let root = sources.blocks.as_path();
    let name = group.disk.name.as_str();
    let partitions = group
        .partitions
        .iter()
        .filter_map(|part| {
            let found = found(sources, mounts, known, &part.name);
            Some(Partition {
                name: part.name.clone(),
                number: part.number?,
                start: part.start,
                size: part.size,
                kind: text_of(root, &part.name, "parttype")
                    .filter(|text| !text.is_empty())
                    .map(|text| type_name(&text)),
                gpt_name: found.gpt_name,
                filesystem: found.filesystem,
                label: found.label,
                uuid: found.uuid,
                mounts: places(mounts, &part.name),
            })
        })
        .collect();
    let free_after =
        group.last().map(|last| group.disk.size.saturating_sub(last.end()).saturating_sub(GPT_END));
    Disk {
        name: name.to_string(),
        model: said(root, name, "device/model"),
        serial: said(root, name, "device/serial"),
        size: group.disk.size,
        removable: flag_of(root, name, "removable"),
        rotational: flag_of(root, name, "queue/rotational"),
        bus: bus_of(name),
        partitions,
        free_after,
        health: health_of(sources, name),
    }
}

/// The profiles `text` lists, a line for each kind of block group.
fn profiles_of(text: &str) -> Profiles {
    let mut profiles = Profiles::default();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let kind = match words.next() {
            Some("data") => &mut profiles.data,
            Some("metadata") => &mut profiles.metadata,
            Some("system") => &mut profiles.system,
            _ => continue,
        };
        kind.extend(words.map(str::to_string));
    }
    profiles
}

/// The mount a volume is told by: one under `/volumes` if it has, else
/// the first it has.
fn chosen(places: &[String]) -> Option<&String> {
    places.iter().find(|path| path.starts_with("/volumes/")).or_else(|| places.first())
}

/// The volume whose folder is `uuid` in the kernel's, with the devices
/// that hold it.
fn volume_of(sources: &Sources, mounts: &[Mount], uuid: &str, devices: Vec<String>) -> Volume {
    let dir = sources.btrfs.join(uuid);
    let line = |file: &str| fs::read_to_string(dir.join(file)).ok().map(|t| t.trim().to_string());
    let count = |file: &str| line(file).and_then(|text| text.parse().ok());
    let held: Vec<&Mount> = mounts
        .iter()
        .filter(|mount| mount.kind == "btrfs" && devices.contains(&mount.source))
        .collect();
    let places: Vec<String> =
        held.iter().map(|mount| df::mount_path(mount).to_string_lossy().into_owned()).collect();
    let mount = chosen(&places).cloned();
    let space = mount.as_deref().and_then(|path| (sources.space)(Path::new(path)));
    Volume {
        uuid: uuid.to_string(),
        label: line("label").filter(|label| !label.is_empty()),
        size: space.map(|space| space.size),
        used: space.map(|space| space.size.saturating_sub(space.free)),
        free: space.map(|space| space.available),
        mount,
        devices,
        disks: count("disks"),
        missing: count("missing"),
        behind: count("behind"),
        profiles: line("profiles").map(|text| profiles_of(&text)),
        holds: HOLDS.into_iter().filter(|held| places.iter().any(|place| place == held)).collect(),
        scrub: scrub::kept_in(&sources.scrubs, uuid),
    }
}

/// The IDs of the Btrfs volumes mounted, in order.
fn volume_ids(sources: &Sources) -> Vec<String> {
    let Ok(entries) = fs::read_dir(&sources.btrfs) else { return Vec::new() };
    let mut ids: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| scrub::is_uuid(name))
        .collect();
    ids.sort();
    ids
}

/// What `storage show` says of the machine.
fn report(sources: &Sources) -> Report {
    let mounts = fs::read(&sources.mounts).map_or_else(|_| Vec::new(), |t| proc::parse_mounts(&t));
    let groups = layout(&sources.blocks);
    let known = known_volumes(sources);
    let disks: Vec<Disk> =
        groups.iter().map(|group| disk_of(sources, &mounts, &known, group)).collect();
    // The disks and the partitions, each with the filesystem found on it.
    let devices: Vec<(&str, Found)> = groups
        .iter()
        .flat_map(|group| std::iter::once(&group.disk).chain(&group.partitions))
        .map(|block| (block.name.as_str(), found(sources, &mounts, &known, &block.name)))
        .collect();
    let volumes = known
        .iter()
        .map(|volume| {
            let uuid = volume.uuid.as_str();
            let holding = devices
                .iter()
                .filter(|(_, found)| {
                    found.filesystem.as_deref() == Some("btrfs")
                        && found.uuid.as_deref() == Some(uuid)
                })
                .map(|(name, _)| (*name).to_string())
                .collect();
            volume_of(sources, &mounts, uuid, holding)
        })
        .collect();
    let filesystems = mounts
        .iter()
        .filter(|mount| mount.kind != "btrfs")
        .filter_map(|mount| {
            let (_, found) = devices.iter().find(|(name, _)| *name == mount.source)?;
            let path = df::mount_path(mount);
            let space = (sources.space)(&path);
            Some(Filesystem {
                mount: path.to_string_lossy().into_owned(),
                kind: found.filesystem.clone(),
                device: mount.source.clone(),
                size: space.map(|space| space.size),
                used: space.map(|space| space.size.saturating_sub(space.free)),
                free: space.map(|space| space.available),
            })
        })
        .collect();
    Report { format: 1, disks, volumes, filesystems }
}

/// The word the JSON has for a value that is one of a few words, as the
/// tables show it.
fn word(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "-".to_string())
}

/// A size as the tables show it, or `-` for none.
fn shown(bytes: Option<u64>) -> String {
    bytes.map_or_else(|| "-".to_string(), |bytes| util::human(bytes, false))
}

/// Rows of cells lined up in columns, the last unpadded; the columns
/// marked in `right` have their cells to the right.
fn lined(rows: &[Vec<String>], right: &[bool]) -> String {
    let columns = right.len();
    let mut widths = vec![0; columns];
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (at, cell) in row.iter().enumerate() {
            if at > 0 {
                line.push(' ');
            }
            let pad =
                if at + 1 == columns { 0 } else { widths[at].saturating_sub(cell.chars().count()) };
            if right[at] {
                line.extend(std::iter::repeat_n(' ', pad));
                line.push_str(cell);
            } else {
                line.push_str(cell);
                line.extend(std::iter::repeat_n(' ', pad));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// How a volume stands: which of its disks are missing or behind.
fn state_of(volume: &Volume) -> String {
    let mut states = Vec::new();
    if let Some(missing) = volume.missing.filter(|&n| n > 0) {
        states.push(format!("{missing} missing"));
    }
    if let Some(behind) = volume.behind.filter(|&n| n > 0) {
        states.push(format!("{behind} behind"));
    }
    if states.is_empty() { "ok".to_string() } else { states.join(", ") }
}

/// How the data of a volume is kept: its profiles, as `raid1` or, while
/// they change, `single/raid1`.
fn profile_of(volume: &Volume) -> String {
    match &volume.profiles {
        Some(profiles) if !profiles.data.is_empty() => profiles.data.join("/"),
        _ => "-".to_string(),
    }
}

/// The report as the tables a person reads: the disks, then the volumes.
fn text(report: &Report) -> String {
    if report.disks.is_empty() && report.volumes.is_empty() {
        return "No disks.\n".to_string();
    }
    let mut out = String::new();
    if !report.disks.is_empty() {
        let mut rows =
            vec![["DISK", "SIZE", "MODEL", "HEALTH", "ROOM"].map(str::to_string).to_vec()];
        for disk in &report.disks {
            rows.push(vec![
                disk.name.clone(),
                util::human(disk.size, false),
                disk.model.clone().unwrap_or_else(|| "-".to_string()),
                disk.health
                    .as_ref()
                    .map_or_else(|| "-".to_string(), |health| word(&health.verdict)),
                shown(disk.free_after.filter(|&bytes| bytes > 0)),
            ]);
        }
        out.push_str(&lined(&rows, &[false, true, false, false, false]));
    }
    if !report.volumes.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        let mut rows = vec![
            ["VOLUME", "MOUNT", "SIZE", "USED", "FREE", "DEVICES", "PROFILE", "STATE", "SCRUB"]
                .map(str::to_string)
                .to_vec(),
        ];
        for volume in &report.volumes {
            rows.push(vec![
                volume.label.clone().unwrap_or_else(|| volume.uuid.clone()),
                volume.mount.clone().unwrap_or_else(|| "-".to_string()),
                shown(volume.size),
                shown(volume.used),
                shown(volume.free),
                if volume.devices.is_empty() { "-".to_string() } else { volume.devices.join(",") },
                profile_of(volume),
                state_of(volume),
                volume.scrub.as_ref().map_or_else(|| "-".to_string(), |kept| word(&kept.result)),
            ]);
        }
        out.push_str(&lined(&rows, &[false, false, true, true, true, false, false, false, false]));
    }
    out
}

/// `storage show [--json]`.
pub fn run(json: bool) -> Result<i32, Fail> {
    let report = report(&Sources::system());
    if json {
        let line = serde_json::to_string(&report).map_err(|error| fail(error.to_string()))?;
        println!("{line}");
    } else {
        print!("{}", text(&report));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::{Value, json};

    use super::super::tests::Blocks;
    use super::*;

    const UUID: &str = "8f0c2a4e-6d1b-4c7e-9a52-3b1f0e5d7c64";
    const LINUX: &str = "0fc63daf-8483-4772-8e79-3d69d8477de4";
    const EFI: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";

    /// What blkid would find on the devices of the machine of [`machine`].
    fn tags(name: &str) -> Vec<(&'static str, String)> {
        let of = |tags: &[(&'static str, &str)]| {
            tags.iter().map(|(tag, value)| (*tag, (*value).to_string())).collect()
        };
        match name {
            "vda1" => of(&[
                ("LABEL", "BOOT"),
                ("UUID", "ABCD-1234"),
                ("TYPE", "vfat"),
                ("PARTLABEL", "EFI"),
            ]),
            "vda2" => of(&[("PARTLABEL", "spare")]),
            "vdb1" => {
                of(&[("LABEL", "LAB"), ("UUID", UUID), ("TYPE", "btrfs"), ("PARTLABEL", "lab-a")])
            }
            "vdc1" => {
                of(&[("LABEL", "LAB"), ("UUID", UUID), ("TYPE", "btrfs"), ("PARTLABEL", "lab-b")])
            }
            "sdb" => of(&[("UUID", "5d0c1f3a-0000-4000-8000-000000000001"), ("TYPE", "ext4")]),
            _ => Vec::new(),
        }
    }

    /// What statvfs would say of the mounts of the machine.
    fn space(path: &Path) -> Option<Space> {
        match path.to_str()? {
            "/volumes/LAB" => Some(Space {
                size: 1_073_741_824,
                free: 429_496_729,
                available: 400_000_000,
                ..Space::default()
            }),
            "/system/boot" => Some(Space {
                size: 66_043_904,
                free: 60_000_000,
                available: 60_000_000,
                ..Space::default()
            }),
            _ => None,
        }
    }

    const MOUNTS: &str = "tmpfs / tmpfs rw\n\
                          bootfs /system bootfs ro\n\
                          vda1 /system/boot fat rw label=BOOT\n\
                          vdb1 /volumes/LAB btrfs rw label=LAB\n\
                          vdb1 /users btrfs rw label=LAB\n\
                          vdb1 /shared btrfs rw label=LAB\n\
                          sdb /mnt/usb ext4 rw\n";

    const WARNING: &str = r#"{"disk":"vdb","model":"Mirror A","serial":"WD-A1","verdict":"warning","reasons":["8 sectors moved to spares, up from 0"],"hours":21804,"temperature_c":38,"moved":8,"waiting":0,"offline":0,"media_errors":null,"used_percent":null,"checked_ms":1763120000000,"rose":{"at_ms":1763080000000,"title":"vdb moved 8 worn-out sectors to spares"}}"#;
    const PASSED: &str = r#"{"disk":"vda","model":"Dukovo Boot","serial":"BOOT0001","verdict":"passed","reasons":[],"hours":120,"temperature_c":null,"moved":null,"waiting":null,"offline":null,"media_errors":0,"used_percent":3,"checked_ms":1763120000000,"rose":null}"#;
    const SCRUB: &str = r#"{"at_ms":1761980000000,"seconds":10512,"bytes":1240000000000,"errors":0,"fixed":0,"unfixable":0,"result":"ok"}"#;
    const RAID1: &str = "data raid1\nmetadata raid1\nsystem raid1\n";

    /// A machine's files: two boot partitions on one disk, a pair of disks
    /// in a Btrfs volume, a stick with a filesystem on the whole of it, the
    /// service's files of two disks and a kept scrub.
    struct Machine {
        blocks: Blocks,
        dir: PathBuf,
    }

    impl Machine {
        fn new() -> Machine {
            static CALLS: AtomicUsize = AtomicUsize::new(0);
            let call = CALLS.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("storage-show-{}-{call}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            for folder in ["btrfs", "health", "scrubs"] {
                fs::create_dir_all(dir.join(folder)).unwrap();
            }
            let blocks = Blocks::new();
            let on = |name: &str, what: &str, text: &str| blocks.put(name, what, text);
            blocks.disk("vda", 4_194_304);
            on("vda", "device/model", "Dukovo Boot");
            on("vda", "device/serial", "BOOT0001");
            on("vda", "removable", "0");
            on("vda", "queue/rotational", "0");
            blocks.partition("vda1", 1, 2048, 131_072);
            on("vda1", "parttype", EFI);
            blocks.partition("vda2", 2, 133_120, 1_048_576);
            on("vda2", "parttype", "11111111-2222-3333-4444-555555555555");
            blocks.disk("vdb", 2_097_152);
            on("vdb", "device/model", "Mirror A");
            on("vdb", "device/serial", "WD-A1");
            on("vdb", "removable", "0");
            on("vdb", "queue/rotational", "1");
            blocks.partition("vdb1", 1, 2048, 1_048_576);
            on("vdb1", "parttype", LINUX);
            blocks.disk("vdc", 2_097_152);
            on("vdc", "device/model", "Mirror B");
            on("vdc", "queue/rotational", "1");
            blocks.partition("vdc1", 1, 2048, 1_048_576);
            on("vdc1", "parttype", LINUX);
            blocks.disk("sdb", 16_384);
            on("sdb", "removable", "1");
            let machine = Machine { blocks, dir };
            machine.volume(UUID, "LAB", "2", "0", "0", Some(RAID1));
            fs::write(machine.dir.join("mounts"), MOUNTS).unwrap();
            fs::write(machine.dir.join("health/vdb.json"), format!("{WARNING}\n")).unwrap();
            fs::write(machine.dir.join("health/vda.json"), format!("{PASSED}\n")).unwrap();
            fs::write(machine.dir.join(format!("scrubs/{UUID}.json")), format!("{SCRUB}\n"))
                .unwrap();
            machine
        }

        /// Gives the kernel's folder of the volume `uuid`.
        fn volume(
            &self,
            uuid: &str,
            label: &str,
            disks: &str,
            missing: &str,
            behind: &str,
            profiles: Option<&str>,
        ) {
            let dir = self.dir.join("btrfs").join(uuid);
            fs::create_dir_all(&dir).unwrap();
            for (file, text) in
                [("label", label), ("disks", disks), ("missing", missing), ("behind", behind)]
            {
                fs::write(dir.join(file), format!("{text}\n")).unwrap();
            }
            match profiles {
                Some(profiles) => fs::write(dir.join("profiles"), profiles).unwrap(),
                None => {
                    let _ = fs::remove_file(dir.join("profiles"));
                }
            }
        }

        fn sources(&self) -> Sources {
            Sources {
                blocks: self.blocks.path().to_path_buf(),
                mounts: self.dir.join("mounts"),
                btrfs: self.dir.join("btrfs"),
                health: self.dir.join("health"),
                scrubs: self.dir.join("scrubs"),
                tags,
                space,
            }
        }

        fn json(&self) -> Value {
            serde_json::to_value(report(&self.sources())).unwrap()
        }
    }

    impl Drop for Machine {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// A value as the compact JSON the command writes.
    fn compact(value: &impl Serialize) -> String {
        serde_json::to_string(value).unwrap()
    }

    /// Whether `text` has each of `keys` once in a row, in that order.
    fn in_order(text: &str, keys: &[&str]) -> bool {
        let mut from = 0;
        for key in keys {
            let Some(at) = text[from..].find(&format!("\"{key}\":")) else { return false };
            from += at + key.len();
        }
        true
    }

    #[test]
    fn the_report_of_a_machine_is_the_golden_file() {
        let machine = Machine::new();
        let golden: Value = serde_json::from_str(include_str!("show.golden.json")).unwrap();
        assert_eq!(machine.json(), golden);
    }

    #[test]
    fn the_fields_are_written_in_the_order_the_page_is_told() {
        let machine = Machine::new();
        let report = report(&machine.sources());
        assert!(!compact(&report).contains('\n'));
        assert!(in_order(&compact(&report), &["format", "disks", "volumes", "filesystems"]));
        // The disk with a partition, a warning and a rose.
        let disk = &report.disks[2];
        assert!(in_order(
            &compact(disk),
            &[
                "name",
                "model",
                "serial",
                "size",
                "removable",
                "rotational",
                "bus",
                "partitions",
                "free_after",
                "health"
            ]
        ));
        assert!(in_order(
            &compact(&disk.partitions[0]),
            &[
                "name",
                "number",
                "start",
                "size",
                "type",
                "gpt_name",
                "filesystem",
                "label",
                "uuid",
                "mounts"
            ]
        ));
        let health = disk.health.as_ref().unwrap();
        assert!(in_order(
            &compact(health),
            &[
                "verdict",
                "checked_ms",
                "hours",
                "temperature_c",
                "moved",
                "waiting",
                "offline",
                "media_errors",
                "used_percent",
                "rose"
            ]
        ));
        assert!(in_order(&compact(health.rose.as_ref().unwrap()), &["at_ms", "title"]));
        let volume = &report.volumes[0];
        assert!(in_order(
            &compact(volume),
            &[
                "uuid", "label", "mount", "size", "used", "free", "devices", "disks", "missing",
                "behind", "profiles", "holds", "scrub"
            ]
        ));
        assert!(in_order(
            &compact(volume.profiles.as_ref().unwrap()),
            &["data", "metadata", "system"]
        ));
        let kept = volume.scrub.as_ref().unwrap();
        assert!(in_order(
            &compact(kept),
            &["at_ms", "seconds", "bytes", "errors", "fixed", "unfixable", "result"]
        ));
        assert!(in_order(
            &compact(&report.filesystems[0]),
            &["mount", "type", "device", "size", "used", "free"]
        ));
    }

    #[test]
    fn a_machine_with_nothing_has_empty_lists() {
        let machine = Machine::new();
        let sources = Sources {
            blocks: machine.dir.join("none"),
            mounts: machine.dir.join("none"),
            btrfs: machine.dir.join("none"),
            health: machine.dir.join("none"),
            scrubs: machine.dir.join("none"),
            tags,
            space,
        };
        let shown = serde_json::to_value(report(&sources)).unwrap();
        assert_eq!(shown, json!({"format": 1, "disks": [], "volumes": [], "filesystems": []}));
        assert_eq!(text(&report(&sources)), "No disks.\n");
    }

    #[test]
    fn a_disk_the_service_has_not_looked_at_or_has_a_bad_file_for_has_no_health() {
        let machine = Machine::new();
        fs::write(machine.dir.join("health/vdc.json"), "{\"disk\":\"vdc\"").unwrap();
        fs::write(machine.dir.join("health/vdb.json"), "not json\n").unwrap();
        let shown = machine.json();
        for disk in ["sdb", "vdb", "vdc"] {
            let found = shown["disks"].as_array().unwrap().iter().find(|d| d["name"] == disk);
            assert_eq!(found.unwrap()["health"], Value::Null, "{disk}");
        }
        assert_eq!(shown["disks"][1]["health"]["verdict"], "passed");
    }

    #[test]
    fn the_room_after_the_last_partition_is_the_disk_less_gpts_end() {
        let machine = Machine::new();
        let shown = machine.json();
        let sectors = |n: u64| n * 512;
        // vda: 4194304 sectors, the last partition ends at 133120 + 1048576.
        assert_eq!(shown["disks"][1]["free_after"], sectors(4_194_304 - 1_181_696 - 33));
        // A partition that fills the disk leaves none, and not a number
        // below none.
        machine.blocks.partition("vdb1", 1, 2048, 2_097_000);
        assert_eq!(machine.json()["disks"][2]["free_after"], 0);
        // The last in the table is not always the last on the disk.
        machine.blocks.partition("vda1", 1, 1_200_000, 100_000);
        assert_eq!(machine.json()["disks"][1]["free_after"], sectors(4_194_304 - 1_300_000 - 33));
        // No partition, no estimate.
        assert_eq!(machine.json()["disks"][0]["free_after"], Value::Null);
    }

    #[test]
    fn a_volume_has_its_devices_by_id_and_the_folders_its_mounts_hold() {
        let machine = Machine::new();
        let shown = machine.json();
        let volume = &shown["volumes"][0];
        assert_eq!(volume["devices"], json!(["vdb1", "vdc1"]));
        assert_eq!(volume["holds"], json!(["/users", "/shared"]));
        // A mount under /volumes is the one told, though it is not first.
        let text =
            "vdb1 /users btrfs rw\nvdb1 /volumes/LAB btrfs rw\nvdb1 /applications btrfs rw\n";
        fs::write(machine.dir.join("mounts"), text).unwrap();
        let shown = machine.json();
        assert_eq!(shown["volumes"][0]["mount"], "/volumes/LAB");
        assert_eq!(shown["volumes"][0]["holds"], json!(["/users", "/applications"]));
        // With no mount under /volumes, the first.
        fs::write(machine.dir.join("mounts"), "vdc1 /elsewhere btrfs rw\nvdb1 /users btrfs rw\n")
            .unwrap();
        let shown = machine.json();
        assert_eq!(shown["volumes"][0]["mount"], "/elsewhere");
        assert_eq!(shown["volumes"][0]["holds"], json!(["/users"]));
        // Without a mount from its devices, none: nothing is made up.
        fs::write(machine.dir.join("mounts"), "vda1 /system/boot fat rw\n").unwrap();
        let shown = machine.json();
        let volume = &shown["volumes"][0];
        for field in ["mount", "size", "used", "free"] {
            assert_eq!(volume[field], Value::Null, "{field}");
        }
        assert_eq!(volume["holds"], json!([]));
    }

    #[test]
    fn a_volume_that_balance_converts_has_two_profiles_in_a_kind() {
        let machine = Machine::new();
        machine.volume(
            UUID,
            "",
            "2",
            "1",
            "3",
            Some("data single raid1\nmetadata dup\nsystem dup\n"),
        );
        let shown = machine.json();
        let volume = &shown["volumes"][0];
        assert_eq!(
            volume["profiles"],
            json!({"data": ["single", "raid1"], "metadata": ["dup"], "system": ["dup"]})
        );
        assert_eq!((&volume["missing"], &volume["behind"]), (&json!(1), &json!(3)));
        // A label that is an empty line is no label.
        assert_eq!(volume["label"], Value::Null);
    }

    #[test]
    fn what_the_kernel_does_not_say_of_a_volume_is_null() {
        let machine = Machine::new();
        machine.volume(UUID, "LAB", "two", "0", "0", None);
        let shown = machine.json();
        let volume = &shown["volumes"][0];
        assert_eq!(volume["profiles"], Value::Null);
        assert_eq!(volume["disks"], Value::Null);
        assert_eq!(volume["missing"], 0);
        // A folder that is not a volume's ID is not a volume.
        fs::create_dir_all(machine.dir.join("btrfs/features")).unwrap();
        assert_eq!(machine.json()["volumes"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_scrub_that_cannot_be_read_whole_is_not_shown() {
        let machine = Machine::new();
        let path = machine.dir.join(format!("scrubs/{UUID}.json"));
        fs::write(&path, "{\"at_ms\":1").unwrap();
        assert_eq!(machine.json()["volumes"][0]["scrub"], Value::Null);
        fs::remove_file(path).unwrap();
        assert_eq!(machine.json()["volumes"][0]["scrub"], Value::Null);
    }

    #[test]
    fn a_device_that_may_not_be_opened_has_what_its_mount_says() {
        let machine = Machine::new();
        let mut sources = machine.sources();
        sources.tags = |_| Vec::new();
        let shown = serde_json::to_value(report(&sources)).unwrap();
        let boot = &shown["disks"][1]["partitions"][0];
        assert_eq!(boot["filesystem"], "fat");
        assert_eq!(boot["label"], "BOOT");
        assert_eq!(boot["uuid"], Value::Null);
        assert_eq!(boot["gpt_name"], Value::Null);
        // Where the kernel does not say a volume's devices, they cannot
        // be told, and the mount is not guessed from the label.
        assert_eq!(shown["volumes"][0]["devices"], json!([]));
        assert_eq!(shown["volumes"][0]["mount"], Value::Null);
        assert_eq!(shown["filesystems"][0]["type"], "fat");
    }

    #[test]
    fn the_kernel_tells_a_volume_s_devices_to_one_who_may_not_open_them() {
        let machine = Machine::new();
        fs::write(machine.dir.join("btrfs").join(UUID).join("devices"), "vdb1\nvdc1\n").unwrap();
        let mut sources = machine.sources();
        sources.tags = |_| Vec::new();
        let shown = serde_json::to_value(report(&sources)).unwrap();
        let volume = &shown["volumes"][0];
        assert_eq!(volume["devices"], json!(["vdb1", "vdc1"]));
        assert_eq!(volume["mount"], "/volumes/LAB");
        assert_eq!(volume["holds"], json!(["/users", "/shared"]));
        // vdc1, from which nothing is mounted, is the volume's: its
        // filesystem, ID and label are the volume's, and its GPT name,
        // which only the disk says, is not told.
        let vdc = shown["disks"].as_array().unwrap().iter().find(|d| d["name"] == "vdc").unwrap();
        let partition = &vdc["partitions"][0];
        assert_eq!(partition["filesystem"], "btrfs");
        assert_eq!(partition["uuid"], UUID);
        assert_eq!(partition["label"], "LAB");
        assert_eq!(partition["gpt_name"], Value::Null);
        assert_eq!(partition["mounts"], json!([]));
        // With the disks opened, the kernel's list changes nothing.
        let opened = machine.json();
        assert_eq!(opened["volumes"][0]["devices"], json!(["vdb1", "vdc1"]));
    }

    #[test]
    fn the_filesystems_are_the_mounts_from_disks_that_are_not_btrfs() {
        let machine = Machine::new();
        let shown = machine.json();
        let names: Vec<_> = shown["filesystems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["mount"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["/system/boot", "/mnt/usb"]);
        // The same device mounted twice is listed twice.
        let text = format!("{MOUNTS}vda1 /elsewhere fat ro\n");
        fs::write(machine.dir.join("mounts"), text).unwrap();
        assert_eq!(machine.json()["filesystems"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn a_type_is_rdisks_name_or_what_the_kernel_says() {
        assert_eq!(type_name(EFI), "efi");
        assert_eq!(type_name(LINUX), "linux");
        assert_eq!(type_name("0x83"), "linux");
        assert_eq!(type_name("0xef"), "efi");
        assert_eq!(type_name("0xda"), "0xda");
        assert_eq!(
            type_name("11111111-2222-3333-4444-555555555555"),
            "11111111-2222-3333-4444-555555555555"
        );
    }

    #[test]
    fn a_bus_is_told_where_the_name_tells_it() {
        for (name, bus) in [
            ("nvme0n1", Some("nvme")),
            ("vda", Some("virtio")),
            ("mmcblk0", Some("sd-card")),
            ("fd0", Some("floppy")),
            ("cd0", Some("optical")),
            ("sda", None),
            ("xyz", None),
        ] {
            assert_eq!(bus_of(name), bus, "{name}");
        }
    }

    #[test]
    fn the_tables_say_each_disk_and_each_volume() {
        let machine = Machine::new();
        let tables = text(&report(&machine.sources()));
        let expected = [
            "DISK SIZE MODEL       HEALTH  ROOM",
            "sdb  8.0M -           -       -",
            "vda  2.0G Dukovo Boot passed  1.5G",
            "vdb  1.0G Mirror A    warning 511M",
            "vdc  1.0G Mirror B    -       511M",
            "",
            "VOLUME MOUNT        SIZE USED FREE DEVICES   PROFILE STATE SCRUB",
            "LAB    /volumes/LAB 1.0G 615M 382M vdb1,vdc1 raid1   ok    ok",
        ];
        assert_eq!(tables.lines().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn a_volume_that_misses_a_disk_or_is_behind_says_so() {
        let machine = Machine::new();
        let mut volume = volume_of(&machine.sources(), &[], UUID, Vec::new());
        assert_eq!(state_of(&volume), "ok");
        volume.missing = Some(1);
        assert_eq!(state_of(&volume), "1 missing");
        volume.behind = Some(2);
        assert_eq!(state_of(&volume), "1 missing, 2 behind");
        volume.missing = Some(0);
        assert_eq!(state_of(&volume), "2 behind");
    }

    #[test]
    fn the_lines_of_a_table_are_cut_at_their_ends() {
        let rows = vec![
            vec!["A".to_string(), "10".to_string(), "x".to_string()],
            vec!["BBB".to_string(), "5".to_string(), String::new()],
        ];
        assert_eq!(lined(&rows, &[false, true, false]), "A   10 x\nBBB  5\n");
    }
}
