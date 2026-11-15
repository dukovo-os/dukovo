// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `storage grow DISK`: gives the last partition of a disk the room after
//! it, and the Btrfs filesystem on that partition the room of the
//! partition.
//!
//! It is the two steps a person would take by hand, in this order:
//! `rdisk --grow DISK` (rdisk(8)) moves the partition's end to the end of
//! the free space, the kernel growing the partition in place; then
//! `device resize ID:max MOUNT` (device(8)) grows the disk of the
//! filesystem that the partition holds. A filesystem is grown, never
//! shrunk, so there is nothing to undo and no trial: growing loses
//! nothing and cannot cut anyone off.
//!
//! The checks come before the first write, so that nothing is grown that
//! the command then refuses: a partition that is not Btrfs is left as it
//! is. When the partition has no room to grow, the filesystem is still
//! given what its partition has, so that a grow that stopped between its
//! two steps is finished by running it again.
//!
//! The steps are in [`grow`], which asks a [`Machine`] for what it reads
//! and does, so that the tests give it a machine of their own.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::{Block, Group, layout};
use crate::args::Args;
use crate::disk_errors;
use crate::util::{self, Fail, fail};
use crate::{blkid, device, df, probe, proc, rdisk};

/// What a partition holds, as blkid finds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Held {
    /// The filesystem's kind (`btrfs`, `swap`, `ext4`), none for a
    /// partition that holds none that blkid knows.
    kind: Option<String>,
    uuid: Option<String>,
}

/// What `grow` reads of the machine and does to it.
trait Machine {
    /// Whether the command runs as root.
    fn is_root(&self) -> bool;
    /// The disks and their partitions.
    fn layout(&self) -> Vec<Group>;
    /// What the partition `name` holds.
    fn held(&self, name: &str) -> Held;
    /// Where the Btrfs filesystem whose ID is `uuid` is mounted.
    fn mount_of(&self, uuid: &str) -> Option<PathBuf>;
    /// The device ID the Btrfs filesystem on `name` gave that disk.
    fn devid(&self, name: &str) -> Option<u64>;
    /// Moves the end of the last partition of `disk` to the end of the free
    /// space after it, as `rdisk --grow` does, and says what it did.
    fn grow_partition(&self, disk: &str) -> Result<(), Fail>;
    /// Grows the disk `devid` of the filesystem mounted at `mount` to its
    /// partition, as `device resize` does, and says what it did.
    fn resize(&self, devid: u64, mount: &Path) -> Result<(), Fail>;
}

/// The machine this runs on.
struct Real;

impl Machine for Real {
    fn is_root(&self) -> bool {
        util::own_user() == 0
    }

    fn layout(&self) -> Vec<Group> {
        layout(Path::new(probe::BLOCKS))
    }

    fn held(&self, name: &str) -> Held {
        let tags = blkid::tags(name);
        let tag = |wanted: &str| {
            tags.iter().find(|(tag, _)| *tag == wanted).map(|(_, value)| value.clone())
        };
        Held { kind: tag("TYPE"), uuid: tag("UUID") }
    }

    fn mount_of(&self, uuid: &str) -> Option<PathBuf> {
        let holders = probe::holding(uuid);
        let mounts = proc::mounts().ok()?;
        mounts
            .iter()
            .find(|mount| mount.kind == "btrfs" && holders.contains(&mount.source))
            .map(df::mount_path)
    }

    fn devid(&self, name: &str) -> Option<u64> {
        device::devid_of(name)
    }

    fn grow_partition(&self, disk: &str) -> Result<(), Fail> {
        rdisk::main(&mut Args::new("rdisk", vec!["--grow".into(), disk.into()])).map(drop)
    }

    fn resize(&self, devid: u64, mount: &Path) -> Result<(), Fail> {
        let items = vec!["resize".into(), format!("{devid}:max").into(), mount.into()];
        device::main(&mut Args::new("device", items)).map(drop)
    }
}

/// The partition `name` of the machine, as it is now.
fn partition_now(machine: &dyn Machine, disk: &str, name: &str) -> Option<Block> {
    machine
        .layout()
        .into_iter()
        .find(|group| group.disk.name == disk)?
        .partitions
        .into_iter()
        .find(|part| part.name == name)
}

/// Grows the last partition of `disk` and the filesystem on it.
fn grow(machine: &dyn Machine, disk: &str, out: &mut dyn Write) -> Result<i32, Fail> {
    if !machine.is_root() {
        return Err(fail(format!("only root grows a disk; try elevate storage grow {disk}")));
    }
    // A name that is not a disk's, a partition's, or a disk that is gone.
    let group = machine
        .layout()
        .into_iter()
        .find(|group| group.disk.name == disk && super::is_disk_name(disk))
        .ok_or_else(|| disk_errors::no_such_disk(disk))?;
    let last = group
        .last()
        .ok_or_else(|| fail(format!("{disk} has no partition to grow: make one with rdisk")))?
        .clone();
    let held = machine.held(&last.name);
    if held.kind.as_deref() != Some("btrfs") {
        let what = match &held.kind {
            Some(kind) => format!("is {kind}"),
            None => "holds no filesystem".to_string(),
        };
        return Err(fail(format!(
            "the last partition of {disk} {what}: storage grows a Btrfs partition only"
        )));
    }
    machine.grow_partition(disk)?;
    let size = partition_now(machine, disk, &last.name).map_or(last.size, |part| part.size);
    let grew = size > last.size;
    let mount = held.uuid.as_deref().and_then(|uuid| machine.mount_of(uuid));
    match mount {
        Some(mount) => {
            let devid = machine.devid(&last.name).ok_or_else(|| {
                fail(format!(
                    "{}: its Btrfs superblock cannot be read, to grow its filesystem",
                    last.name
                ))
            })?;
            machine.resize(devid, &mount)?;
            if grew {
                writeln!(
                    out,
                    "{} and the Btrfs filesystem on it are {} now.",
                    last.name,
                    util::human(size, false)
                )?;
            }
        }
        None if grew => writeln!(
            out,
            "{} is not mounted, so its Btrfs filesystem is not grown: run storage grow {disk} \
             again once it is.",
            last.name
        )?,
        None => {}
    }
    if !grew {
        writeln!(out, "{disk} has no room after its last partition.")?;
    }
    Ok(0)
}

/// `storage grow DISK`.
pub fn run(disk: &str) -> Result<i32, Fail> {
    let mut out = io::stdout().lock();
    grow(&Real, disk, &mut out)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::super::tests::Blocks;
    use super::*;

    /// A machine that keeps what was done to it.
    struct Fake {
        root: bool,
        blocks: Blocks,
        /// What each partition holds: its name, kind and ID.
        held: Vec<(&'static str, Option<&'static str>, Option<&'static str>)>,
        mount: Option<PathBuf>,
        devid: Option<u64>,
        /// The sectors the last partition gains when it is grown.
        gain: u64,
        /// What the steps asked for, in order.
        done: RefCell<Vec<String>>,
        fail_resize: bool,
    }

    impl Fake {
        /// A disk of 2048 sectors with a boot partition and a Btrfs one,
        /// which ends at sector 1024, and a mounted filesystem.
        fn new() -> Fake {
            let blocks = Blocks::new();
            blocks.disk("vdb", 2048);
            blocks.partition("vdb1", 1, 64, 128);
            blocks.partition("vdb2", 2, 192, 832);
            blocks.disk("vdc", 512);
            Fake {
                root: true,
                blocks,
                held: vec![("vdb1", Some("vfat"), None), ("vdb2", Some("btrfs"), Some("U-1"))],
                mount: Some(PathBuf::from("/volumes/LAB")),
                devid: Some(2),
                gain: 900,
                done: RefCell::new(Vec::new()),
                fail_resize: false,
            }
        }

        fn done(&self) -> Vec<String> {
            self.done.borrow().clone()
        }
    }

    impl Machine for Fake {
        fn is_root(&self) -> bool {
            self.root
        }

        fn layout(&self) -> Vec<Group> {
            layout(self.blocks.path())
        }

        fn held(&self, name: &str) -> Held {
            let found = self.held.iter().find(|(held, _, _)| *held == name);
            Held {
                kind: found.and_then(|(_, kind, _)| kind.map(str::to_string)),
                uuid: found.and_then(|(_, _, uuid)| uuid.map(str::to_string)),
            }
        }

        fn mount_of(&self, uuid: &str) -> Option<PathBuf> {
            self.done.borrow_mut().push(format!("mount {uuid}"));
            self.mount.clone()
        }

        fn devid(&self, name: &str) -> Option<u64> {
            self.done.borrow_mut().push(format!("devid {name}"));
            self.devid
        }

        fn grow_partition(&self, disk: &str) -> Result<(), Fail> {
            self.done.borrow_mut().push(format!("rdisk --grow {disk}"));
            let last =
                self.layout().into_iter().find(|g| g.disk.name == disk).unwrap().last().cloned();
            if let (Some(last), true) = (last, self.gain > 0) {
                let (start, sectors) = (last.start / 512, last.size / 512);
                let number = last.number.unwrap();
                self.blocks.partition(&last.name, number, start, sectors + self.gain);
            }
            Ok(())
        }

        fn resize(&self, devid: u64, mount: &Path) -> Result<(), Fail> {
            self.done.borrow_mut().push(format!("device resize {devid}:max {}", mount.display()));
            if self.fail_resize { Err(fail("the filesystem is read-only".into())) } else { Ok(()) }
        }
    }

    fn run_on(machine: &Fake, disk: &str) -> (Result<i32, Fail>, String) {
        let mut out = Vec::new();
        let result = grow(machine, disk, &mut out);
        (result, String::from_utf8(out).unwrap())
    }

    #[test]
    fn the_partition_is_grown_and_then_its_filesystem_to_the_partition() {
        let machine = Fake::new();
        let (result, said) = run_on(&machine, "vdb");
        assert_eq!(result.unwrap(), 0);
        assert_eq!(
            machine.done(),
            ["rdisk --grow vdb", "mount U-1", "devid vdb2", "device resize 2:max /volumes/LAB"]
        );
        let size = util::human((832 + 900) * 512, false);
        assert_eq!(said, format!("vdb2 and the Btrfs filesystem on it are {size} now.\n"));
    }

    #[test]
    fn only_root_grows_a_disk_and_nothing_is_done_before_that() {
        let mut machine = Fake::new();
        machine.root = false;
        let (result, said) = run_on(&machine, "vdb");
        let error = result.unwrap_err();
        assert_eq!(error.message, "only root grows a disk; try elevate storage grow vdb");
        assert_eq!((error.code, error.usage), (1, false));
        assert!(machine.done().is_empty());
        assert!(said.is_empty());
    }

    #[test]
    fn a_name_that_is_no_disk_is_dsk_101() {
        let machine = Fake::new();
        for name in ["vdz", "vdb1", "", "../vdb", "VDB", "vdb\n"] {
            let (result, _) = run_on(&machine, name);
            let error = result.unwrap_err();
            assert_eq!(error.code, 1, "{name:?}");
            assert!(error.message.ends_with("(DSK-101)"), "{name:?}: {}", error.message);
            assert!(error.message.contains(&format!("no disk called {name}")), "{name:?}");
        }
        assert!(machine.done().is_empty());
    }

    #[test]
    fn a_disk_with_no_partition_has_none_to_grow() {
        let machine = Fake::new();
        let (result, _) = run_on(&machine, "vdc");
        let error = result.unwrap_err();
        assert_eq!(error.message, "vdc has no partition to grow: make one with rdisk");
        assert!(machine.done().is_empty());
    }

    #[test]
    fn a_last_partition_that_is_not_btrfs_is_refused_before_anything_is_grown() {
        for (kind, said) in [
            (
                Some("swap"),
                "the last partition of vdb is swap: storage grows a Btrfs partition only",
            ),
            (
                Some("ext4"),
                "the last partition of vdb is ext4: storage grows a Btrfs partition only",
            ),
            (
                None,
                "the last partition of vdb holds no filesystem: storage grows a Btrfs partition only",
            ),
        ] {
            let mut machine = Fake::new();
            machine.held[1] = ("vdb2", kind, None);
            let (result, out) = run_on(&machine, "vdb");
            let error = result.unwrap_err();
            assert_eq!((error.message.as_str(), error.code), (said, 1));
            assert!(machine.done().is_empty(), "nothing was grown");
            assert!(out.is_empty());
        }
    }

    #[test]
    fn the_partition_that_ends_last_is_the_one_grown() {
        // Partition 1 is later on the disk than partition 2.
        let machine = Fake::new();
        machine.blocks.partition("vdb1", 1, 1100, 100);
        let (result, _) = run_on(&machine, "vdb");
        let error = result.unwrap_err();
        assert!(
            error.message.starts_with("the last partition of vdb is vfat"),
            "{}",
            error.message
        );
    }

    #[test]
    fn with_no_room_it_says_so_and_still_gives_the_filesystem_what_its_partition_has() {
        let mut machine = Fake::new();
        machine.gain = 0;
        let (result, said) = run_on(&machine, "vdb");
        assert_eq!(result.unwrap(), 0);
        assert_eq!(said, "vdb has no room after its last partition.\n");
        // A grow that stopped after its first step is finished by this one.
        assert_eq!(machine.done().last().unwrap(), "device resize 2:max /volumes/LAB");
        // A second grow of one that grew is the same.
        let mut machine = Fake::new();
        assert!(run_on(&machine, "vdb").0.is_ok());
        machine.gain = 0;
        let (result, said) = run_on(&machine, "vdb");
        assert_eq!(result.unwrap(), 0);
        assert_eq!(said, "vdb has no room after its last partition.\n");
    }

    #[test]
    fn a_filesystem_that_is_not_mounted_is_told_to_grow_when_it_is() {
        let mut machine = Fake::new();
        machine.mount = None;
        let (result, said) = run_on(&machine, "vdb");
        assert_eq!(result.unwrap(), 0);
        assert_eq!(machine.done(), ["rdisk --grow vdb", "mount U-1"]);
        assert!(
            said.starts_with("vdb2 is not mounted, so its Btrfs filesystem is not grown"),
            "{said}"
        );
        assert!(said.contains("run storage grow vdb again"), "{said}");
        // With no room it has nothing to add to the one line.
        let mut machine = Fake::new();
        machine.mount = None;
        machine.gain = 0;
        let (_, said) = run_on(&machine, "vdb");
        assert_eq!(said, "vdb has no room after its last partition.\n");
    }

    #[test]
    fn a_failed_resize_fails_the_command_and_says_what_failed() {
        let mut machine = Fake::new();
        machine.fail_resize = true;
        let (result, said) = run_on(&machine, "vdb");
        assert_eq!(result.unwrap_err().message, "the filesystem is read-only");
        assert!(said.is_empty());
    }

    #[test]
    fn a_filesystem_whose_disk_id_cannot_be_read_is_not_resized_blind() {
        let mut machine = Fake::new();
        machine.devid = None;
        let (result, _) = run_on(&machine, "vdb");
        assert!(
            result.unwrap_err().message.starts_with("vdb2: its Btrfs superblock cannot be read")
        );
        assert!(!machine.done().iter().any(|step| step.starts_with("device resize")));
    }
}
