// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Host tests of checking filesystems (ADR 0305): what `mkfs.btrfs` and
//! this crate make and write, [`check`] finds whole, as Linux's `btrfs
//! check` does; what is spoiled on purpose, it finds wrong, as `btrfs
//! check` does.

use std::format;
use std::string::String;
use std::vec::Vec;

use radix_block::DeviceRef;
use radix_block::ram::RamDisk;
use radix_vfs::mem;
use radix_vfs::{FileSystem, Kind, Slice};

use super::files::{noise, rich_tree};
use super::rw::mkfs_with;
use super::write::{btrfs_check, image_of};
use super::{DIR, FILE, LINK, open};
use crate::check::{self, PHASES, Report};
use crate::format::{Key, item, tree, u32_at, u64_at};
use crate::fs::BtrfsFs;
use crate::make::{self, Options};
use crate::tree::Cursor;

const MIB: u64 = 1 << 20;

/// What `check` finds in `image`, checking the data too if `data`.
fn report(image: &[u8], data: bool) -> Report {
    let volume = open(image).unwrap();
    let mut phases = Vec::new();
    let report = check::check(&volume, data, &mut |n, name| phases.push((n, name.to_owned()))).unwrap();
    let names: Vec<_> = PHASES.iter().enumerate().map(|(i, p)| (i + 1, (*p).to_owned())).collect();
    assert_eq!(phases, names);
    report
}

/// Asserts `check` and `btrfs check` both find `image` whole.
fn clean(image: &[u8], what: &str) -> Report {
    let (whole, text) = btrfs_check(image);
    assert!(whole, "{what}: btrfs check failed:\n{text}");
    let report = report(image, true);
    assert!(report.problems.is_empty(), "{what}: {:#?}", report.problems);
    report
}

/// A filesystem this crate made and wrote: subvolumes, a snapshot that
/// then changed, files compressed and not, links, a file removed.
fn written() -> Vec<u8> {
    let size = 256 * MIB;
    let disk = mem::arc(RamDisk::new(size / 512, false)).unwrap();
    let device: DeviceRef = disk.clone();
    make::make(&device, &Options::new(b"written", 1_793_592_777)).unwrap();
    let fs = BtrfsFs::mount_writable(device).unwrap();
    fs.set_compress(true);
    let root = fs.root();
    root.make_subvolume("@home").unwrap();
    let home = root.lookup("@home").unwrap();
    let dir = home.create("docs", Kind::Directory, DIR).unwrap();
    for (n, len) in [0usize, 10, 3000, 5000, 70_000, 1 << 20].into_iter().enumerate() {
        let file = dir.create(&format!("f{n}"), Kind::File, FILE).unwrap();
        let data = if n % 2 == 0 { noise(n as u64, len) } else { "text ".repeat(len / 5).into_bytes() };
        assert_eq!(file.write(0, &Slice(&data)).unwrap(), data.len());
    }
    let f1 = dir.lookup("f1").unwrap();
    home.link("hard", &f1).unwrap();
    home.make_link("soft", "docs/f2", LINK).unwrap();
    root.snapshot("@home.snap", &home, false).unwrap();
    let snap = root.lookup("@home.snap").unwrap();
    let file = snap.lookup("docs").unwrap().lookup("f4").unwrap();
    file.write(1000, &Slice(&noise(9, 100_000))).unwrap();
    dir.remove("f3").unwrap();
    fs.sync().unwrap();
    drop((file, snap, f1, dir, home, root, fs));
    image_of(&disk, size as usize)
}

#[test]
fn what_is_whole_is_found_whole() {
    let made = |o: &Options<'_>| {
        let disk = mem::arc(RamDisk::new(256 * MIB / 512, false)).unwrap();
        let device: DeviceRef = disk.clone();
        make::make(&device, o).unwrap();
        image_of(&disk, (256 * MIB) as usize)
    };
    let fresh = clean(&made(&Options::new(b"", 0)), "made");
    assert_eq!((fresh.tree_blocks, fresh.data_extents, fresh.subvolumes), (9, 0, 1));
    let o = Options { dup: false, nodesize: 4096, ..Options::new(b"", 0) };
    clean(&made(&o), "made, one copy, 4 KiB nodes");
    let written = clean(&written(), "written");
    assert_eq!(written.subvolumes, 3);
    assert!(written.data_extents > 5 && written.files > 8, "{written:?}");

    let dir = rich_tree();
    for args in [
        &[][..],
        &["--compress", "zstd"],
        &["-m", "single", "--csum", "xxhash"],
        &["-O", "block-group-tree", "--subvol", "rw:dir"],
        &["--nodesize", "4096", "--csum", "sha256"],
    ] {
        let image = mkfs_with(args, &dir);
        clean(&image, &format!("mkfs.btrfs {args:?}"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Changes item `key` of tree `id` in `image` with `f`, in every copy of
/// its leaf, and seals the leaf again: damage its checksum does not show.
fn spoil(image: &mut [u8], id: u64, key: Key, f: impl Fn(&mut [u8])) {
    let volume = open(image).unwrap();
    let root = match id {
        tree::ROOT => volume.root_tree(),
        tree::CHUNK => volume.chunk_root(),
        _ => volume.tree(id).unwrap().unwrap(),
    };
    let cursor = Cursor::seek(&volume, &root, &key).unwrap();
    assert_eq!(cursor.key(), Some(key));
    let (bytenr, slot) = cursor.leaf().unwrap();
    let sites: Vec<u64> = volume.place(bytenr).unwrap().copies().iter().map(|s| s.at).collect();
    let (nodesize, csum) = (volume.superblock().nodesize as usize, volume.superblock().csum_type);
    drop(cursor);
    drop(volume);
    for at in sites {
        let block = &mut image[at as usize..at as usize + nodesize];
        let header = 101 + slot * 25;
        let (offset, size) = (u32_at(block, header + 17) as usize, u32_at(block, header + 21) as usize);
        f(&mut block[101 + offset..101 + offset + size]);
        let sum = csum.sum(&block[32..]);
        block[..32].copy_from_slice(&sum);
    }
}

/// The key of the first item of tree `id` in `image` of `kind` whose data
/// `pick` takes.
fn first(image: &[u8], id: u64, kind: u8, pick: impl Fn(&[u8]) -> bool) -> Key {
    let volume = open(image).unwrap();
    let root = if id == tree::CHUNK { volume.chunk_root() } else { volume.tree(id).unwrap().unwrap() };
    let mut cursor = Cursor::seek(&volume, &root, &Key::new(0, 0, 0)).unwrap();
    while let Some((key, data)) = cursor.get() {
        if key.kind == kind && pick(data) {
            return key;
        }
        cursor.advance().unwrap();
    }
    panic!("no item of type {kind} in tree {id}");
}

fn add_u64(d: &mut [u8], at: usize, delta: u64) {
    let v = u64_at(d, at).wrapping_add(delta);
    d[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

#[test]
fn what_is_spoiled_is_found_as_btrfs_check_finds_it() {
    let base = written();
    let file = first(&base, tree::FS, item::INODE_ITEM, |_| true);
    let group = first(&base, tree::EXTENT, item::BLOCK_GROUP_ITEM, |_| true);
    let block = first(&base, tree::EXTENT, item::METADATA_ITEM, |_| true);
    let device = first(&base, tree::CHUNK, item::DEV_ITEM, |_| true);
    let mode = |d: &[u8]| u32_at(d, 52) & 0o170_000 == 0o100_000;
    type Spoil = (&'static str, u64, Key, fn(&mut [u8]));
    let cases: [Spoil; 5] = [
        ("a directory's size", tree::FS, file, |d| add_u64(d, 16, 2)),
        ("a block group's bytes used", tree::EXTENT, group, |d| add_u64(d, 0, 4096)),
        ("a tree block's references", tree::EXTENT, block, |d| add_u64(d, 0, 1)),
        ("a device's bytes in chunks", tree::CHUNK, device, |d| add_u64(d, 16, MIB)),
        ("a link count", 0, file, |d| {
            let n = u32_at(d, 40) + 1;
            d[40..44].copy_from_slice(&n.to_le_bytes());
        }),
    ];
    for (what, id, key, f) in cases {
        let mut image = base.clone();
        let (id, key) = if id == 0 {
            // A file's inode, in the subvolume made.
            (256, first(&base, 256, item::INODE_ITEM, mode))
        } else {
            (id, key)
        };
        spoil(&mut image, id, key, f);
        let (whole, text) = btrfs_check(&image);
        assert!(!whole, "{what}: btrfs check found nothing:\n{text}");
        let found = report(&image, false);
        assert!(!found.problems.is_empty(), "{what}: check found nothing, btrfs check:\n{text}");
    }

    // A data checksum: found when the data is read.
    let mut image = base.clone();
    let sums = first(&base, tree::CSUM, item::EXTENT_CSUM, |_| true);
    spoil(&mut image, tree::CSUM, sums, |d| d[0] ^= 0xff);
    let quiet = report(&image, false).problems;
    assert!(quiet.is_empty(), "{quiet:#?}");
    let found = report(&image, true).problems;
    assert!(found.iter().any(|p| p.contains("bad checksum")), "{found:#?}");
    assert!(!btrfs_check(&image).0);

    // A bad copy of a tree block, the other good: found, and read past.
    let mut image = base.clone();
    let volume = open(&image).unwrap();
    let place = volume.place(volume.root_tree().bytenr).unwrap();
    let second = place.copies()[1].at as usize;
    drop(volume);
    image[second + 200] ^= 0x55;
    let found = report(&image, false).problems;
    assert!(found.len() == 1 && found[0].contains("copy 2") && found[0].contains("checksum"), "{found:#?}");
    let _: String = found.concat();
}
