// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! storage(8): the disks and the filesystems on them, and the room a disk
//! has gained.
//!
//! `storage show` says what each disk is, its partitions, what its health
//! is, which Btrfs volumes there are with the disks each is made of and
//! how it keeps its copies, and the other filesystems mounted, as a short
//! table or as JSON, which the web console's Storage page reads. It joins
//! what `lsblk`, `blkid`, `df`, `diskhealth` and `scrub` give, from the
//! same files they read, so that a person who may not open a disk sees it
//! all the same. `storage grow DISK` gives the last partition of a disk
//! the room after it, and the Btrfs filesystem on it the room of the
//! partition. The command line is a list of words and the option
//! `--json`, which may come before the words or after them: [`parse`]
//! reads it, and [`main`] gives the words to the command they name.
//!
//! When a disk is missing from the answer, run `lsblk` for what the kernel
//! has, and `diskhealth` for what the disk says of itself.

mod grow;
mod show;

pub(crate) use show::BTRFS;

use std::fs;
use std::path::Path;

use crate::args::{Arg, Args};
use crate::probe;
use crate::util::{Fail, usage};

const USAGE: &str = "Usage: storage [show] [--json]\n       \
                     storage grow DISK\n  \
                     --json  the disks, volumes and filesystems as JSON\n  \
                     DISK    a disk's name, as `storage show` gives it (sda, vdb, nvme0n1)";

/// The bytes in a sector as `/kernel/block` counts them.
const SECTOR: u64 = 512;

/// Whether `name` is a name `/kernel/block` can give a disk: one to 32
/// lower-case letters and digits (`vda`, `sda`, `nvme0n1`, `mmcblk0`). It
/// says nothing of whether the machine has such a disk, which the
/// commands look up; the web console checks it before it asks them, so
/// that a name never reaches a path.
pub(crate) fn is_disk_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// A disk or a partition, as `/kernel/block` has it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Block {
    name: String,
    /// Its size in bytes.
    size: u64,
    /// Where it begins on its disk, in bytes: nothing for a disk.
    start: u64,
    /// Its number in its disk's table, for a partition.
    number: Option<u32>,
}

impl Block {
    /// Where it ends on its disk, in bytes.
    fn end(&self) -> u64 {
        self.start.saturating_add(self.size)
    }
}

/// A disk with its partitions, in the order of their numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Group {
    disk: Block,
    partitions: Vec<Block>,
}

impl Group {
    /// The partition that ends last on the disk, which is the one a grow
    /// gives the room after it.
    fn last(&self) -> Option<&Block> {
        self.partitions.iter().max_by_key(|part| part.end())
    }
}

/// What the file `what` of `name` in `root` says, without the end of its
/// line; `None` if it is not there.
fn text_of(root: &Path, name: &str, what: &str) -> Option<String> {
    fs::read_to_string(root.join(name).join(what)).ok().map(|text| text.trim().to_string())
}

/// The number the file `what` of `name` in `root` has.
fn number_of(root: &Path, name: &str, what: &str) -> Option<u64> {
    text_of(root, name, what)?.parse().ok()
}

/// The disks `root` lists, in the order of their names, each with its
/// partitions. An entry whose size is not given is left out, as is a
/// partition that does not say where it begins or that is on no disk
/// listed: the kernel always gives these, so what lacks them is not a
/// device this can describe.
fn layout(root: &Path) -> Vec<Group> {
    let Ok(entries) = fs::read_dir(root) else { return Vec::new() };
    let mut disks = Vec::new();
    let mut partitions = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(sectors) = number_of(root, &name, "size") else { continue };
        let size = sectors.saturating_mul(SECTOR);
        match number_of(root, &name, "partition") {
            None => disks.push(Block { name, size, start: 0, number: None }),
            Some(number) => {
                let (Ok(number), Some(start)) =
                    (u32::try_from(number), number_of(root, &name, "start"))
                else {
                    continue;
                };
                let start = start.saturating_mul(SECTOR);
                partitions.push(Block { name, size, start, number: Some(number) });
            }
        }
    }
    disks.sort_by(|a, b| a.name.cmp(&b.name));
    partitions.sort_by_key(|part| part.number);
    disks
        .into_iter()
        .map(|disk| {
            let own = partitions
                .iter()
                .filter(|part| probe::disk_of(&part.name) == disk.name)
                .cloned()
                .collect();
            Group { disk, partitions: own }
        })
        .collect()
}

/// What the command line says: its words, and the option.
#[derive(Debug, PartialEq, Eq)]
struct Command {
    /// The operands, in order: the command's name and what it takes.
    words: Vec<String>,
    json: bool,
}

/// Reads the command line, which is `Ok(None)` for `--help`.
fn parse(args: &mut Args) -> Result<Option<Command>, Fail> {
    let mut command = Command { words: Vec::new(), json: false };
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => command.json = true,
            Arg::Long(name) if name == "help" => return Ok(None),
            Arg::Operand(word) => command.words.push(word.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(Some(command))
}

/// storage(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let Some(command) = parse(args)? else {
        println!("{USAGE}");
        return Ok(0);
    };
    let words: Vec<&str> = command.words.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["show"] => show::run(command.json),
        ["grow", _] if command.json => Err(usage(format!("{USAGE}\n--json goes with show"))),
        ["grow", disk] => grow::run(disk),
        _ => Err(usage(format!("{USAGE}\nno such command: {}", words.join(" ")))),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A folder of the files `/kernel/block` has, in a place of its own
    /// for each call, as the tests run at once.
    pub(super) struct Blocks(std::path::PathBuf);

    impl Blocks {
        pub(super) fn new() -> Blocks {
            static CALLS: AtomicUsize = AtomicUsize::new(0);
            let call = CALLS.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("storage-blocks-{}-{call}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Blocks(dir)
        }

        pub(super) fn path(&self) -> &Path {
            &self.0
        }

        /// Adds the file `what` of `name`.
        pub(super) fn put(&self, name: &str, what: &str, text: &str) {
            let path = self.0.join(name).join(what);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("{text}\n")).unwrap();
        }

        /// Adds a disk of `sectors` sectors.
        pub(super) fn disk(&self, name: &str, sectors: u64) {
            self.put(name, "size", &sectors.to_string());
        }

        /// Adds partition `number` of a disk, from sector `start`, of
        /// `sectors` sectors.
        pub(super) fn partition(&self, name: &str, number: u32, start: u64, sectors: u64) {
            self.put(name, "size", &sectors.to_string());
            self.put(name, "partition", &number.to_string());
            self.put(name, "start", &start.to_string());
        }
    }

    impl Drop for Blocks {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn read(items: &[&str]) -> Result<Option<Command>, Fail> {
        parse(&mut Args::new("storage", items.iter().map(Into::into).collect()))
    }

    fn command(words: &[&str], json: bool) -> Option<Command> {
        Some(Command { words: words.iter().map(ToString::to_string).collect(), json })
    }

    #[test]
    fn a_disk_name_is_one_to_32_lower_case_letters_and_digits() {
        for good in ["vda", "sda", "nvme0n1", "mmcblk0", "a", "x".repeat(32).as_str()] {
            assert!(is_disk_name(good), "{good}");
        }
        for bad in [
            "",
            "VDA",
            "../x",
            "vda/",
            "a b",
            "vda\n",
            "vda\0",
            "v-a",
            "vdá",
            "x".repeat(33).as_str(),
        ] {
            assert!(!is_disk_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_words_and_the_option_are_read_wherever_the_option_comes() {
        assert_eq!(read(&[]).unwrap(), command(&[], false));
        assert_eq!(read(&["show"]).unwrap(), command(&["show"], false));
        assert_eq!(read(&["--json", "show"]).unwrap(), command(&["show"], true));
        assert_eq!(read(&["show", "--json"]).unwrap(), command(&["show"], true));
        assert_eq!(read(&["grow", "vdb"]).unwrap(), command(&["grow", "vdb"], false));
        assert_eq!(read(&["--help"]).unwrap(), None);
    }

    #[test]
    fn an_option_it_does_not_have_or_a_command_it_does_not_have_is_a_mistake() {
        for items in [&["--all"][..], &["-x"], &["--json=yes"]] {
            assert!(read(items).unwrap_err().usage, "{items:?}");
        }
        for words in [
            &["format", "vdb"][..],
            &["grow"],
            &["grow", "vda", "vdb"],
            &["show", "vda"],
            &["grow", "vdb", "--json"],
        ] {
            let mut args = Args::new("storage", words.iter().map(Into::into).collect());
            assert!(main(&mut args).unwrap_err().usage, "{words:?}");
        }
    }

    #[test]
    fn the_disks_come_by_name_with_their_partitions_by_number() {
        let blocks = Blocks::new();
        blocks.disk("vdb", 2048);
        blocks.disk("vda", 4096);
        blocks.partition("vda2", 2, 2048, 1024);
        blocks.partition("vda1", 1, 64, 1984);
        blocks.disk("nvme0n1", 512);
        blocks.partition("nvme0n1p1", 1, 2, 100);
        let found = layout(blocks.path());
        let names: Vec<_> = found
            .iter()
            .map(|g| {
                let parts: Vec<_> = g.partitions.iter().map(|p| p.name.as_str()).collect();
                (g.disk.name.as_str(), parts)
            })
            .collect();
        assert_eq!(
            names,
            [("nvme0n1", vec!["nvme0n1p1"]), ("vda", vec!["vda1", "vda2"]), ("vdb", vec![])]
        );
        let vda = &found[1];
        assert_eq!(vda.disk.size, 4096 * 512);
        assert_eq!((vda.partitions[0].start, vda.partitions[0].size), (64 * 512, 1984 * 512));
        assert_eq!(vda.last().unwrap().name, "vda2");
        assert_eq!(vda.last().unwrap().end(), (2048 + 1024) * 512);
    }

    #[test]
    fn what_the_kernel_does_not_describe_whole_is_left_out() {
        let blocks = Blocks::new();
        blocks.disk("vda", 4096);
        blocks.put("vdb", "removable", "0");
        // A partition that does not say where it begins, and one of no
        // disk the kernel lists.
        blocks.put("vda1", "size", "10");
        blocks.put("vda1", "partition", "1");
        blocks.partition("vdc1", 1, 2, 10);
        blocks.partition("vda2", 2, 64, 10);
        let found = layout(blocks.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].partitions.len(), 1);
        assert_eq!(found[0].partitions[0].name, "vda2");
        assert!(layout(&blocks.path().join("nowhere")).is_empty());
    }
}
