// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Where the slots and the slot state are: the Dukovo root this system
//! started from, mounted whole at `/volumes/<disk>` (spark(8)), and the
//! EFI system partitions of the disks it is on, each with its own two
//! copies of the slot state beside GRUB's menu (ADR 0452).
//!
//! GRUB writes the copies of the partition it started from; the system
//! writes every partition's. So the state the system goes by is that of
//! the partition whose newest copy says it started the running slot, or
//! failing that the first partition's, and every write goes to all of
//! them, each over its own older copy, so that a mirror's second disk
//! starts the same slot when the first is gone.

use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use slots::about::About;
use slots::state::{self, COPIES, Copy};
use slots::{Slot, State};

use crate::util::{self, Fail, fail};
use crate::{blockdev, probe};

/// The kernel's command line, which names the root and the slot.
const CMDLINE: &str = "/kernel/cmdline";

/// Where the root's top level is mounted: `/volumes/<disk>`.
const VOLUMES: &str = "/volumes";

/// The type of an EFI system partition, as `/kernel/block/<name>/parttype`
/// gives it: in a GPT, and in an MBR.
const EFI_SYSTEM: [&str; 2] = ["c12a7328-f81f-11d2-ba4b-00a0c93ec93b", "0xef"];

/// The subvolume a stage builds the spare slot in, until it is sealed.
pub const BUILDING: &str = "@system.new";

/// The lock that keeps two writers of the slot state apart: the stage and
/// the health check. It is in memory, so that a restart frees it.
const LOCK: &str = "/system/live/sysupdate.lock";

/// The Dukovo root this system started from.
pub struct Root {
    /// Its name in `/volumes` and `/devices`: `vda3`.
    pub disk: String,
    /// Its Btrfs's UUID.
    pub uuid: String,
    /// The slot that runs, which GRUB named on the kernel's command line.
    pub running: Slot,
}

impl Root {
    /// The path of `name` in the root's top level.
    pub fn path(&self, name: &str) -> PathBuf {
        Path::new(VOLUMES).join(&self.disk).join(name)
    }

    /// The path of `slot`'s subvolume.
    pub fn slot(&self, slot: Slot) -> PathBuf {
        self.path(slot.subvolume())
    }

    /// What `slot` says of itself, if it is there and says.
    pub fn about(&self, slot: Slot) -> Option<About> {
        let bytes = std::fs::read(self.slot(slot).join(slots::about::PATH)).ok()?;
        About::decode(&bytes)
    }
}

/// The root this system started from: the Btrfs the kernel's command line
/// names by its UUID, mounted at `/volumes/<disk>`, which holds the slot
/// GRUB started.
///
/// # Errors
///
/// If there is none, as on the live system.
pub fn root() -> Result<Root, Fail> {
    let line = std::fs::read_to_string(CMDLINE).unwrap_or_default();
    let boot = slots::cmdline::parse(&line);
    let named = line
        .split_whitespace()
        .take_while(|&word| word != "--")
        .filter_map(|word| word.strip_prefix("root=UUID="))
        .last()
        .map(str::to_ascii_lowercase);
    let running = boot.system();
    let mut disks: Vec<String> = std::fs::read_dir(VOLUMES)
        .map_err(|err| util::failed(&VOLUMES, err))?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    disks.sort();
    for disk in disks {
        let top = Path::new(VOLUMES).join(&disk);
        if !top.join(running.subvolume()).is_dir() || !top.join("@state").is_dir() {
            continue;
        }
        let Some(uuid) = btrfs_uuid(&disk) else { continue };
        if named.as_ref().is_some_and(|named| *named != uuid) {
            continue;
        }
        return Ok(Root { disk, uuid, running });
    }
    Err(fail(
        "this system did not start from a Dukovo root on a disk, so it has no slots to update"
            .into(),
    ))
}

/// The UUID of the Btrfs on `/devices/<name>`, if it holds one.
pub(crate) fn btrfs_uuid(name: &str) -> Option<String> {
    let mut read = probe::reader(name)?;
    let found = probe::filesystem(&mut read)?;
    if found.kind == "btrfs" { found.uuid } else { None }
}

/// The EFI system partitions of the disks the root is on, by their names
/// in `/devices`, in order: one, or one a disk on a mirror.
///
/// # Errors
///
/// If the disks cannot be listed.
pub fn partitions(root: &Root) -> Result<Vec<String>, Fail> {
    let devices = probe::devices().map_err(|err| util::failed(&probe::BLOCKS, err))?;
    let mut disks: Vec<&str> = Vec::new();
    for (name, partition) in &devices {
        if *partition && btrfs_uuid(name).as_deref() == Some(root.uuid.as_str()) {
            let disk = probe::disk_of(name);
            if !disks.contains(&disk) {
                disks.push(disk);
            }
        }
    }
    let efi = |name: &str| {
        probe::block_text(name, "parttype")
            .is_some_and(|kind| EFI_SYSTEM.contains(&kind.to_ascii_lowercase().as_str()))
    };
    Ok(devices
        .iter()
        .filter(|(name, partition)| *partition && disks.contains(&probe::disk_of(name)))
        .filter(|(name, _)| efi(name))
        .map(|(name, _)| name.clone())
        .collect())
}

/// An EFI system partition's two copies of the slot state, as read.
#[derive(Clone, Debug)]
pub struct Copies {
    /// The partition's name in `/devices`.
    pub partition: String,
    /// Each copy, if it is whole.
    pub first: Option<State>,
    pub second: Option<State>,
}

impl Copies {
    /// The newer whole copy, and which it is.
    pub fn newest(&self) -> Option<(State, Copy)> {
        state::newest(self.first, self.second)
    }
}

/// The partition `/devices/<name>`, opened as the disk engine's device.
fn device(name: &str, write: bool) -> Result<disk::Device, Fail> {
    let path = format!("/devices/{name}");
    let opened = blockdev::open(OsStr::new(&path), write, None)?;
    if write && !opened.writable {
        return Err(fail(format!("{path} refuses writes")));
    }
    opened.device().map_err(|err| util::failed(&path, err))
}

/// The copies of the slot state on the EFI system partition `name`.
///
/// # Errors
///
/// If the partition cannot be read, or holds no FAT.
pub fn read(name: &str) -> Result<Copies, Fail> {
    let copy = |path: &str| -> Result<Option<State>, Fail> {
        let bytes = disk::esp::read_file(device(name, false)?, path, slots::envblock::BLOCK)
            .map_err(|why| fail(format!("/devices/{name}: {why}")))?;
        Ok(bytes.and_then(|bytes| State::decode(&bytes)))
    };
    Ok(Copies { partition: name.to_string(), first: copy(COPIES[0])?, second: copy(COPIES[1])? })
}

/// The root's EFI system partitions with their copies, and the state the
/// system goes by: the newest of the partition that started the running
/// slot, or of the first with a whole copy, or, with none, the running
/// slot alone, as an install leaves it.
///
/// # Errors
///
/// If the root's disks have no EFI system partition, or one cannot be
/// read.
pub fn state(root: &Root) -> Result<(Vec<Copies>, State), Fail> {
    let copies: Vec<Copies> =
        partitions(root)?.iter().map(|name| read(name)).collect::<Result<_, _>>()?;
    if copies.is_empty() {
        return Err(fail(format!(
            "the disks of the root {} have no EFI system partition, where the slot state is",
            root.uuid
        )));
    }
    let newest: Vec<State> = copies.iter().filter_map(|c| c.newest().map(|(s, _)| s)).collect();
    let state = newest
        .iter()
        .find(|state| state.boot == Some(root.running))
        .or_else(|| newest.first())
        .copied()
        .unwrap_or(State { default: root.running, ..State::installed() });
    Ok((copies, state))
}

/// Writes `state` on each partition of `copies` over its older copy, as
/// the generation after its newer one, or over both when it has no whole
/// one.
///
/// # Errors
///
/// If no partition could be written; those that could not are named.
pub fn write(copies: &[Copies], state: State) -> Result<(), Fail> {
    let mut written = 0;
    let mut problems = Vec::new();
    for esp in copies {
        let done = match esp.newest() {
            Some((newer, copy)) => {
                let next = State { generation: newer.next().generation, ..state };
                put(&esp.partition, copy.other().path(), next)
            }
            None => {
                let first = State { generation: 0, ..state };
                put(&esp.partition, COPIES[0], first)
                    .and_then(|()| put(&esp.partition, COPIES[1], first))
            }
        };
        match done {
            Ok(()) => written += 1,
            Err(err) => problems.push(err.message),
        }
    }
    match (written, problems.is_empty()) {
        (_, true) => Ok(()),
        (0, false) => Err(fail(problems.join("; "))),
        (_, false) => {
            eprintln!("sysupdate: {}", problems.join("; "));
            Ok(())
        }
    }
}

/// Writes `state` over both copies of every partition, one after the
/// other, so that no copy is left that names what it no longer does: a
/// torn second write leaves the first, and a torn first stops before
/// the second. A stage forgets the spare slot this way before it removes
/// what it holds.
///
/// # Errors
///
/// If the copies cannot be read again, or no partition could be written.
pub fn write_both(root: &Root, state: State) -> Result<(), Fail> {
    for _ in 0..2 {
        let (copies, _) = self::state(root)?;
        write(&copies, state)?;
    }
    Ok(())
}

/// Writes `state` as the copy `path` of the partition `name`, in place.
fn put(name: &str, path: &str, state: State) -> Result<(), Fail> {
    disk::esp::overwrite_file(device(name, true)?, path, &state.encode())
        .map_err(|why| fail(format!("/devices/{name}: {path}: {why}")))
}

/// Sets Rescue's count of the starts not seen through back to 0 on each
/// EFI system partition named in `partitions` that has one above 0, so
/// that the boot menu starts Rescue only after three more such starts in
/// a row (ADR 0460). A partition with no count, as an install before
/// Rescue leaves, is passed over. Returns the partitions it could not
/// write, with why.
pub fn reset_rescue<'a>(partitions: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut problems = Vec::new();
    for name in partitions {
        let read = device(name, false).and_then(|device| {
            disk::esp::read_file(device, slots::rescue::COUNT, slots::envblock::BLOCK)
                .map_err(|why| fail(format!("/devices/{name}: {why}")))
        });
        let starts = match read {
            Ok(Some(block)) => slots::rescue::decode(&block),
            Ok(None) => continue,
            Err(why) => {
                problems.push(why.message);
                continue;
            }
        };
        if starts == Some(0) {
            continue;
        }
        let written = device(name, true).and_then(|device| {
            disk::esp::overwrite_file(device, slots::rescue::COUNT, &slots::rescue::encode(0))
                .map_err(|why| fail(format!("/devices/{name}: {}: {why}", slots::rescue::COUNT)))
        });
        if let Err(why) = written {
            problems.push(why.message);
        }
    }
    problems
}

/// The lock on the slot state, held while it lives.
pub struct Lock;

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(LOCK);
    }
}

/// Takes the lock on the slot state, or says who holds it. A lock whose
/// process is gone is taken over.
///
/// # Errors
///
/// If another sysupdate holds it.
pub fn lock() -> Result<Lock, Fail> {
    for _ in 0..2 {
        match OpenOptions::new().write(true).create_new(true).open(LOCK) {
            Ok(mut file) => {
                let _ = writeln!(file, "{}", std::process::id());
                return Ok(Lock);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let holder = std::fs::read_to_string(LOCK).unwrap_or_default();
                let pid = holder.trim();
                if !pid.is_empty() && Path::new("/processes").join(pid).exists() {
                    return Err(fail(format!("another sysupdate is at work, process {pid}")));
                }
                let _ = std::fs::remove_file(LOCK);
            }
            Err(err) => return Err(util::failed(&LOCK, err)),
        }
    }
    Err(fail(format!("cannot take the lock {LOCK}")))
}

/// Takes the lock on the slot state once a stage running now lets it go,
/// waiting up to `most`: the health check waits out a stage started
/// soon after the system came up.
///
/// # Errors
///
/// If it is held all that time.
pub fn lock_within(most: std::time::Duration) -> Result<Lock, Fail> {
    let start = std::time::Instant::now();
    loop {
        match lock() {
            Ok(lock) => return Ok(lock),
            Err(why) if start.elapsed() >= most => return Err(why),
            Err(_) => std::thread::sleep(std::time::Duration::from_secs(1)),
        }
    }
}

/// What `slot` is called in messages: `slot B (Dukovo 0.15.0)`.
pub fn named(root: &Root, slot: Slot) -> String {
    let letter = slot.letter().to_ascii_uppercase();
    match root.about(slot) {
        Some(about) => format!("slot {letter} (Dukovo {})", about.version),
        None => format!("slot {letter}"),
    }
}
