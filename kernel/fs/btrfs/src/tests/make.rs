// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Host tests of making filesystems (ADR 0304): what [`make`] makes,
//! Linux's `btrfs check` finds whole, holds the items `mkfs.btrfs`'s
//! filesystems hold, and mounts to be written, by this crate and then
//! checked again.

use std::format;
use std::process::Command;
use std::string::{String, ToString};
use std::vec;
use std::vec::Vec;

use radix_block::DeviceRef;
use radix_block::ram::RamDisk;
use radix_vfs::mem::{self, KArc};
use radix_vfs::{Error, FileSystem, Kind, Slice};

use super::files::{contents, mount, noise};
use super::write::{btrfs_check, image_of, mkfs_sized, write_sparse};
use super::{DIR, FILE, LINK, scratch, tool};
use crate::chunk::kind;
use crate::csum::CsumType;
use crate::fs::BtrfsFs;
use crate::make::{self, MIN_BYTES, Options};
use crate::node::BtrfsNode;

const MIB: u64 = 1 << 20;

/// Options with UUIDs of their own, made at a set time.
fn options() -> Options<'static> {
    Options::new(b"made", 1_793_592_777)
}

/// A filesystem made on a disk of `bytes` with `o`.
fn made(bytes: u64, o: &Options<'_>) -> KArc<RamDisk> {
    let disk = mem::arc(RamDisk::new(bytes / 512, false)).unwrap();
    let device: DeviceRef = disk.clone();
    make::make(&device, o).unwrap();
    disk
}

/// Asserts `btrfs check` finds the first `len` bytes of `disk` whole.
fn checks(disk: &RamDisk, len: u64, what: &str) {
    let (whole, text) = btrfs_check(&image_of(disk, len as usize));
    assert!(whole, "{what}: btrfs check failed:\n{text}");
}

/// What `btrfs inspect-internal` prints of the image at `path`.
fn inspect(args: &[&str], path: &std::path::Path) -> String {
    let out = Command::new(tool("btrfs"))
        .arg("inspect-internal")
        .args(args)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "btrfs inspect-internal: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The items of each tree `dump-tree` prints, by their keys' types and
/// their object IDs where those are names, not numbers (the block
/// addresses differ, and the UUIDs).
fn shape(dump: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in dump.lines() {
        let line = line.trim();
        if line.ends_with("tree key")
            || line.contains(" tree key (")
            || line.starts_with("chunk tree")
            || line.starts_with("root tree")
        {
            out.push(line.split(" key").next().unwrap_or(line).to_string());
        }
        let Some(key) = line.split("key (").nth(1).filter(|_| line.starts_with("item ")) else {
            continue;
        };
        let mut fields = key.split(' ');
        let (objectid, kind) = (fields.next().unwrap_or(""), fields.next().unwrap_or(""));
        let objectid =
            if objectid.starts_with(|c: char| c.is_ascii_digit()) { "#" } else { objectid };
        out.push(format!("  {objectid} {kind}"));
    }
    out
}

/// The lines of `dump-super` that say what a filesystem is, not which
/// one or where its trees are.
fn features(dump: &str) -> Vec<String> {
    const SAME: [&str; 14] = [
        "csum_type",
        "csum_size",
        "flags",
        "magic",
        "total_bytes",
        "sectorsize",
        "nodesize",
        "leafsize",
        "stripesize",
        "root_dir",
        "num_devices",
        "compat_ro_flags",
        "incompat_flags",
        "dev_item.sector_size",
    ];
    let mut out = Vec::new();
    for line in dump.lines() {
        let name = line.split_whitespace().next().unwrap_or("");
        let flag = line.starts_with("\t\t\t") && line.contains(['(', '|', ')']);
        if SAME.contains(&name) || flag {
            out.push(line.split('[').next().unwrap_or(line).trim_end().to_string());
        }
    }
    out
}

#[test]
fn what_is_made_is_what_mkfs_btrfs_makes() {
    let size = 512 * MIB;
    let ours = image_of(&made(size, &options()), size as usize);
    let (whole, text) = btrfs_check(&ours);
    assert!(whole, "btrfs check failed:\n{text}");
    let theirs = mkfs_sized(size, &["-L", "made"]);
    let (ours_at, theirs_at) = (scratch("ours"), scratch("theirs"));
    write_sparse(&ours_at, &ours);
    write_sparse(&theirs_at, &theirs);
    let dump = |path| inspect(&["dump-tree"], path);
    let (a, b) = (dump(&ours_at), dump(&theirs_at));
    // The free-space tree and the extent tree name the blocks where they
    // are: their shapes differ with them.
    let trees = |dump: &str| {
        let mut out = Vec::new();
        let mut keep = true;
        // The device tree's error counts, which mkfs.btrfs makes since
        // 6.7, and this crate keeps once there are some.
        for line in shape(dump).into_iter().filter(|l| !l.contains("PERSISTENT_ITEM")) {
            if !line.starts_with(' ') {
                keep = !line.contains("free space") && !line.contains("extent");
            }
            if keep {
                out.push(line);
            }
        }
        out
    };
    assert_eq!(trees(&a), trees(&b), "ours:\n{a}\n\nmkfs.btrfs's:\n{b}");
    let supers = |path| features(&inspect(&["dump-super", "-f"], path));
    assert_eq!(supers(&ours_at), supers(&theirs_at));
    assert!(
        a.contains("label made") || inspect(&["dump-super"], &ours_at).contains("label\t\t\tmade")
    );
    let _ = std::fs::remove_file(&ours_at);
    let _ = std::fs::remove_file(&theirs_at);
}

#[test]
fn chunks_are_mkfs_btrfss_sizes() {
    let metadata = |bytes: u64| make::layout(bytes, true).unwrap().groups[1].length;
    // What mkfs.btrfs 6.17 gives devices of these sizes.
    assert_eq!(metadata(109 * MIB), 32 * MIB);
    assert_eq!(metadata(512 * MIB), 32 * MIB);
    assert_eq!(metadata(700 * MIB), 36_700_160);
    assert_eq!(metadata(1 << 30), 53_673_984);
    assert_eq!(metadata(2 << 30), 107_347_968);
    assert_eq!(metadata(2500 * MIB), 131_072_000);
    assert_eq!(metadata(3 << 30), 256 * MIB);
    assert_eq!(metadata(50 << 30), 256 * MIB);
    assert_eq!(metadata(51 << 30), 1 << 30);
    assert_eq!(metadata(2 << 40), 1 << 30);
    let layout = make::layout(512 * MIB, true).unwrap();
    let [system, meta, data] = layout.groups;
    assert_eq!(
        (system.flags, system.length, system.copies),
        (kind::SYSTEM | kind::DUP, 8 * MIB, 2)
    );
    assert_eq!((meta.flags, meta.copies), (kind::METADATA | kind::DUP, 2));
    assert_eq!((data.flags, data.length, data.copies), (kind::DATA, 8 * MIB, 1));
    assert_eq!(layout.allocated(), 88 * MIB);
    assert_eq!(make::layout(MIN_BYTES - 4096, true), Err(Error::NoSpace));
    let single = make::layout(512 * MIB, false).unwrap().groups[1];
    assert_eq!((single.flags, single.copies), (kind::METADATA, 1));
}

#[test]
fn sizes_options_and_the_superblocks_copies_check() {
    // The smallest device; one where the copy at 64 MiB falls where the
    // first tree block would, at the start of the metadata chunk's second
    // copy (17 MiB + 47 MiB); one where it falls in the first copy.
    for bytes in [MIN_BYTES, 940 * MIB, 3 << 30] {
        let o = options();
        if bytes == 940 * MIB {
            assert_eq!(make::layout(bytes, true).unwrap().groups[1].length, 47 * MIB);
        }
        checks(&made(bytes, &o), bytes, &format!("{bytes} bytes"));
    }
    let o = Options { nodesize: 4096, csum: CsumType::Xxhash64, dup: false, ..options() };
    checks(&made(256 * MIB, &o), 256 * MIB, "4 KiB nodes, xxhash, one copy");
    let o = Options { nodesize: 64 << 10, csum: CsumType::Sha256, ..options() };
    checks(&made(256 * MIB, &o), 256 * MIB, "64 KiB nodes, sha256");
    let o = Options { csum: CsumType::Blake2b, label: b"", ..options() };
    checks(&made(256 * MIB, &o), 256 * MIB, "blake2b, no label");
    // The copy at 256 GiB, on a device that reaches past it.
    let disk = made(257 << 30, &options());
    let mut copy = vec![0; 4096];
    disk.dump(256 << 30, &mut copy);
    assert_eq!(&copy[0x40..0x48], b"_BHRfS_M");
    assert_eq!(u64::from_le_bytes(copy[0x30..0x38].try_into().unwrap()), 256 << 30);
}

#[test]
fn what_is_made_refuses_what_it_cannot_make() {
    let device = |bytes: u64, read_only| -> DeviceRef {
        mem::arc(RamDisk::new(bytes / 512, read_only)).unwrap()
    };
    let o = options();
    assert_eq!(make::make(&device(100 * MIB, false), &o).err(), Some(Error::NoSpace));
    assert_eq!(make::make(&device(256 * MIB, true), &o).err(), Some(Error::ReadOnly));
    for nodesize in [2048, 12288, 128 << 10] {
        let o = Options { nodesize, ..options() };
        assert_eq!(make::make(&device(256 * MIB, false), &o).err(), Some(Error::InvalidArgument));
    }
    let long = [b'a'; 256];
    let o = Options { label: &long, ..options() };
    assert_eq!(make::make(&device(256 * MIB, false), &o).err(), Some(Error::InvalidArgument));
    let o = Options { label: &long[..255], ..options() };
    assert!(make::make(&device(256 * MIB, false), &o).is_ok());
    // Nothing is written to a device refused.
    let disk = mem::arc(RamDisk::new(100 * MIB / 512, false)).unwrap();
    let as_device: DeviceRef = disk.clone();
    let _ = make::make(&as_device, &options());
    assert_eq!(disk.requests(), 0);
}

#[test]
fn what_is_made_is_written_and_read_back() {
    let size = 256 * MIB;
    let disk = made(size, &options());
    let device: DeviceRef = disk.clone();
    let fs = BtrfsFs::mount_writable(device).unwrap();
    fs.set_compress(true);
    let root = fs.root();
    // What ./ob image makes: subvolumes, one read-only, files, a link.
    for name in ["@system.a", "@settings", "@users"] {
        root.make_subvolume(name).unwrap();
    }
    let system = root.lookup("@system.a").unwrap();
    let dir = system.create("bin", Kind::Directory, DIR).unwrap();
    let text = "Dukovo ".repeat(40_000).into_bytes();
    let random = noise(7, 300_000);
    for (name, data) in [("text", &text), ("noise", &random)] {
        let file = dir.create(name, Kind::File, FILE).unwrap();
        assert_eq!(file.write(0, &Slice(data)).unwrap(), data.len());
        file.set_times(Some(1_700_000_000_000_000_000), Some(1_700_000_000_000_000_000)).unwrap();
    }
    system.make_link("sh", "bin/text", LINK).unwrap();
    radix_vfs::downcast::<BtrfsNode>(&*system).unwrap().make_read_only().unwrap();
    // Not the top of a subvolume.
    assert_eq!(
        radix_vfs::downcast::<BtrfsNode>(&*dir).unwrap().make_read_only(),
        Err(Error::InvalidArgument)
    );
    fs.sync().unwrap();
    drop((dir, system, root, fs));
    checks(&disk, size, "written");

    let fs = mount(&image_of(&disk, size as usize)).unwrap();
    let system = fs.root().lookup("@system.a").unwrap();
    assert!(system.read_only());
    let bin = system.lookup("bin").unwrap();
    assert_eq!(contents(&*bin.lookup("text").unwrap(), 65536).unwrap(), text);
    assert_eq!(contents(&*bin.lookup("noise").unwrap(), 65536).unwrap(), random);
    assert!(!fs.root().lookup("@settings").unwrap().read_only());
    // The text was compressed, the noise not.
    let used = fs.volume().superblock().bytes_used;
    assert!(used < (random.len() + text.len() / 2) as u64 + MIB, "{used}");
}
