// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The images made, read back by the kernel's own readers (radix-iso9660
//! through Rock Ridge, Joliet and the plain tree, and radix-udf) and by
//! isoinfo and udfinfo (`apt install genisoimage udftools`; `ob doctor`
//! checks for them).

use std::process::Command;
use std::sync::Arc;

use radix_block::DeviceRef;
use radix_block::ram::RamDisk;
use radix_vfs::mem;
use radix_vfs::{FileSystem, Kind as VfsKind, NodeRef, SliceMut};

use super::*;

/// 2026-11-10 12:00:00 UTC.
const WHEN: i64 = 1_794_312_000;

/// Bytes that differ from one place to the next.
fn pattern(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

fn file(name: &str, bytes: Vec<u8>) -> Node {
    let size = bytes.len() as u64;
    Node::new(name, Kind::File { size, source: Source::Bytes(Arc::from(bytes)), link: None }, WHEN)
}

fn dir(name: &str, children: Vec<Node>) -> Node {
    Node::new(name, Kind::Dir(children), WHEN)
}

fn link(name: &str, target: &str) -> Node {
    Node::new(name, Kind::Symlink(target.as_bytes().to_vec()), WHEN)
}

/// The long name and the deep path the tree holds.
fn long() -> String {
    format!("{}.longname", "n".repeat(200))
}

const DEEP: &str = "a/b/c/d/e/f/g/h/i/j";

/// A tree of files of many sizes, folders deeper than ISO 9660's eight
/// levels, names long, Unicode and alike in their first eight
/// characters, links, and two names of one file.
fn tree() -> Node {
    let mut deep = file("bottom.txt", b"deep".to_vec());
    for name in DEEP.split('/').rev() {
        deep = dir(name, vec![deep]);
    }
    let mut shared = file("first", pattern(3000, 9));
    shared.kind = match shared.kind {
        Kind::File { size, source, .. } => Kind::File { size, source, link: Some((1, 42)) },
        other => other,
    };
    let mut second = shared.clone();
    second.name = b"second".to_vec();
    let mut script = file("run.sh", b"#!/bin/sh\necho hi\n".to_vec());
    script.mode = 0o750;
    script.uid = 1000;
    script.gid = 100;
    script.modified = WHEN - 86_400;
    dir(
        "",
        vec![
            file("empty", Vec::new()),
            file("one", vec![7]),
            file("Mixed Case.txt", pattern(2048, 1)),
            file("photos-from-the-summer-2026.jpeg", pattern(5000, 2)),
            file("photos-from-the-summer-2025.jpeg", pattern(5001, 3)),
            file("Ünïcödé 日本.txt", b"unicode".to_vec()),
            file(&long(), b"long".to_vec()),
            script,
            dir(
                "big",
                vec![file("whole", pattern(1 << 20, 4)), file("odd", pattern(300 * 1024 + 7, 5))],
            ),
            dir(
                "links",
                vec![
                    link("relative", "../big/odd"),
                    link("absolute", "/system/bin/sh"),
                    shared,
                    second,
                ],
            ),
            dir("empty dir", Vec::new()),
            deep,
        ],
    )
}

/// The image of `tree` as `options` say, in memory.
fn image(tree: Node, options: Options) -> Vec<u8> {
    let image = Image::new(tree, options).unwrap();
    let mut out = Vec::new();
    image.write(&mut out).unwrap();
    assert_eq!(out.len() as u64, image.bytes());
    out
}

fn device(image: &[u8]) -> DeviceRef {
    let disk = mem::arc(RamDisk::with_blocks(image.len() as u64 / 512, false, 2048)).unwrap();
    disk.load(0, image).unwrap();
    disk
}

fn walk(dir: &NodeRef, path: &str) -> NodeRef {
    let mut node = dir.clone();
    for name in path.split('/').filter(|n| !n.is_empty()) {
        node = node.lookup(name).unwrap_or_else(|e| panic!("{path}: {e:?}"));
    }
    node
}

fn read_all(node: &NodeRef) -> Vec<u8> {
    let mut all = Vec::new();
    let mut buf = vec![0u8; 3000];
    loop {
        let n = node.read(all.len() as u64, &mut SliceMut(&mut buf)).unwrap();
        if n == 0 {
            return all;
        }
        all.extend_from_slice(&buf[..n]);
    }
}

fn names(dir: &NodeRef) -> Vec<String> {
    let mut all = Vec::new();
    dir.list(0, &mut |_, name, _| {
        all.push(name.to_string());
        true
    })
    .unwrap();
    all.sort();
    all
}

fn target(node: &NodeRef) -> String {
    String::from_utf8(radix_vfs::link_target(&**node).unwrap().clone()).unwrap()
}

/// What a tree with every name and attribute must read as, through
/// `root`.
fn check_full(root: &NodeRef, posix: bool) {
    let long = long();
    let mut want = vec![
        "Mixed Case.txt",
        "big",
        "empty",
        "empty dir",
        "links",
        "one",
        "photos-from-the-summer-2025.jpeg",
        "photos-from-the-summer-2026.jpeg",
        "run.sh",
        "Ünïcödé 日本.txt",
        "a",
        &long,
    ];
    want.sort_unstable();
    assert_eq!(names(root), want);
    assert_eq!(read_all(&walk(root, "big/whole")), pattern(1 << 20, 4));
    assert_eq!(read_all(&walk(root, "big/odd")), pattern(300 * 1024 + 7, 5));
    assert_eq!(read_all(&walk(root, "photos-from-the-summer-2025.jpeg")), pattern(5001, 3));
    assert_eq!(read_all(&walk(root, "Ünïcödé 日本.txt")), b"unicode");
    assert_eq!(read_all(&walk(root, &long)), b"long");
    assert_eq!(read_all(&walk(root, "empty")), b"");
    assert_eq!(read_all(&walk(root, "one")), [7]);
    assert_eq!(read_all(&walk(root, &format!("{DEEP}/bottom.txt"))), b"deep");
    assert!(names(&walk(root, "empty dir")).is_empty());
    assert_eq!(read_all(&walk(root, "links/first")), pattern(3000, 9));
    assert_eq!(read_all(&walk(root, "links/second")), pattern(3000, 9));
    assert_eq!(walk(root, "big").stat().kind, VfsKind::Directory);
    assert_eq!(walk(root, "big/odd").stat().modified / 1_000_000_000, WHEN as u64);
    if posix {
        assert_eq!(target(&walk(root, "links/relative")), "../big/odd");
        assert_eq!(target(&walk(root, "links/absolute")), "/system/bin/sh");
        let script = walk(root, "run.sh");
        assert_eq!(script.access().mode, 0o750);
        assert_eq!((script.access().owner, script.access().group), (1000, 100));
        assert_eq!(script.stat().modified / 1_000_000_000, (WHEN - 86_400) as u64);
    }
}

#[test]
fn rock_ridge_reads_back_every_name_mode_owner_time_and_link() {
    let options = Options::new("Summer 2026", WHEN);
    let bytes = image(tree(), options);
    let fs = radix_iso9660::IsoFs::mount(device(&bytes), true).unwrap();
    let mut label = String::new();
    fs.label(&mut label).unwrap();
    // Joliet's label, as given, before the primary one's upper case.
    assert_eq!(label, "Summer 2026");
    check_full(&fs.root(), true);
}

#[test]
fn joliet_alone_reads_back_unicode_names() {
    let options = Options { rock: false, ..Options::new("Summer 2026", WHEN) };
    let bytes = image(tree(), options);
    let fs = radix_iso9660::IsoFs::mount(device(&bytes), true).unwrap();
    let root = fs.root();
    // Joliet cuts names to 64 characters, its extension kept.
    let cut = format!("{}.longname", "n".repeat(64 - ".longname".len()));
    let names = names(&root);
    assert!(names.contains(&cut), "{names:?}");
    assert!(names.contains(&"Ünïcödé 日本.txt".to_string()));
    assert_eq!(read_all(&walk(&root, "photos-from-the-summer-2026.jpeg")), pattern(5000, 2));
    assert_eq!(read_all(&walk(&root, &format!("{DEEP}/bottom.txt"))), b"deep");
}

#[test]
fn the_plain_tree_has_unique_names_of_eight_and_three() {
    let options = Options { rock: false, joliet: false, ..Options::new("x", WHEN) };
    let bytes = image(tree(), options);
    let fs = radix_iso9660::IsoFs::mount(device(&bytes), true).unwrap();
    let root = fs.root();
    let names = names(&root);
    // Shown in lower case, as Linux shows them; the two photos' first
    // eight characters are alike, so the second's end in a number.
    assert!(names.contains(&"photos_f.jpe".to_string()), "{names:?}");
    assert!(names.contains(&"photos_0.jpe".to_string()), "{names:?}");
    assert!(names.contains(&"mixed_ca.txt".to_string()), "{names:?}");
    let photos: Vec<_> = names.iter().filter(|n| n.starts_with("photos")).collect();
    let bytes: Vec<_> = photos.iter().map(|n| read_all(&walk(&root, n)).len()).collect();
    let mut sorted = bytes.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, [5000, 5001]);
}

#[test]
fn udf_reads_back_the_same_tree() {
    let options = Options { udf: true, ..Options::new("Summer 2026", WHEN) };
    let bytes = image(tree(), options);
    let fs = radix_udf::UdfFs::mount(device(&bytes), true).unwrap();
    let mut label = String::new();
    fs.label(&mut label).unwrap();
    assert_eq!(label, "Summer 2026");
    check_full(&fs.root(), true);
    // The ISO 9660 tree beside it is whole too.
    let iso = radix_iso9660::IsoFs::mount(device(&bytes), true).unwrap();
    check_full(&iso.root(), true);
}

#[test]
fn the_reference_tools_read_the_images() {
    let dir = std::env::temp_dir().join(format!("disk-disc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("disc.iso");
    let options = Options { udf: true, ..Options::new("Summer 2026", WHEN) };
    std::fs::write(&path, image(tree(), options)).unwrap();
    let run = |program: &str, args: &[&str]| {
        let out =
            Command::new(program).args(args).arg(&path).output().unwrap_or_else(|_| {
                panic!("{program} is missing: apt install genisoimage udftools")
            });
        assert!(out.status.success(), "{program}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let described = run("isoinfo", &["-d", "-i"]);
    assert!(described.contains("Volume id: SUMMER_2026"), "{described}");
    assert!(described.contains("Joliet with UCS level 3 found"), "{described}");
    assert!(described.contains("Rock Ridge signatures version 1 found"), "{described}");
    let listed = run("isoinfo", &["-R", "-f", "-i"]);
    assert!(listed.contains(&format!("/{DEEP}/bottom.txt")), "{listed}");
    assert!(listed.contains("/Mixed Case.txt"), "{listed}");
    let listed = run("isoinfo", &["-J", "-f", "-i"]);
    assert!(listed.contains("/big/whole"), "{listed}");
    let udf = run("udfinfo", &[]);
    assert!(udf.contains("label=Summer 2026"), "{udf}");
    assert!(udf.contains("udfrev=2.01"), "{udf}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_file_past_4_gib_is_several_extents() {
    // Laid out, not written: its bytes are never read.
    let size = 5 << 30;
    let big = Node::new(
        "big.bin",
        Kind::File { size, source: Source::Path("/nonexistent".into()), link: None },
        WHEN,
    );
    let options = Options { udf: true, ..Options::new("big", WHEN) };
    let image = Image::new(dir("", vec![big]), options).unwrap();
    assert!(image.sectors() > size / SECTOR);
    // The root's records: ".", "..", then BIG.BIN;1 twice, the first
    // marked as having more.
    let root = (image.layout.iso_dirs[0].0 * SECTOR) as usize;
    let mut at = root;
    let mut records = Vec::new();
    while image.head[at] != 0 {
        let len = usize::from(image.head[at]);
        let id = &image.head[at + 33..at + 33 + usize::from(image.head[at + 32])];
        let size = u32::from_le_bytes(image.head[at + 10..at + 14].try_into().unwrap());
        records.push((id.to_vec(), size, image.head[at + 25]));
        at += len;
    }
    assert_eq!(records.len(), 4);
    assert_eq!(records[2], (b"BIG.BIN;1".to_vec(), 0xffff_f800, 0x80));
    assert_eq!(records[3], (b"BIG.BIN;1".to_vec(), (size - 0xffff_f800) as u32, 0));
}

#[test]
fn names_and_links_too_long_are_refused() {
    let name = "x".repeat(256);
    assert!(matches!(
        Image::new(dir("", vec![file(&name, Vec::new())]), Options::new("x", WHEN)),
        Err(Error::TooLong(_))
    ));
    let path = "p/".repeat(600);
    assert!(matches!(
        Image::new(dir("", vec![link("l", &path)]), Options::new("x", WHEN)),
        Err(Error::TooLong(_))
    ));
}

#[test]
fn a_folder_is_scanned_with_what_a_disc_cannot_hold_left_out() {
    let root = std::env::temp_dir().join(format!("disk-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/b"), b"bee").unwrap();
    std::fs::write(root.join("a"), b"a").unwrap();
    std::fs::hard_link(root.join("a"), root.join("a2")).unwrap();
    std::os::unix::fs::symlink("sub/b", root.join("l")).unwrap();
    let pipe = root.join("pipe");
    let made = Command::new("mkfifo").arg(&pipe).status().is_ok_and(|s| s.success());
    let (tree, left) = Node::scan(&root).unwrap();
    assert_eq!(left, if made { vec![pipe] } else { Vec::new() });
    let Kind::Dir(children) = &tree.kind else { panic!("a folder") };
    let names: Vec<_> =
        children.iter().map(|c| String::from_utf8_lossy(&c.name).into_owned()).collect();
    assert_eq!(names, ["a", "a2", "l", "sub"]);
    let bytes = image(tree, Options::new("scan", WHEN));
    let fs = radix_iso9660::IsoFs::mount(device(&bytes), true).unwrap();
    let r = fs.root();
    assert_eq!(read_all(&walk(&r, "sub/b")), b"bee");
    assert_eq!(read_all(&walk(&r, "a2")), b"a");
    assert_eq!(target(&walk(&r, "l")), "sub/b");
    assert_eq!(walk(&r, "a").stat().links, 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_file_that_changed_since_it_was_scanned_fails_the_write() {
    let root = std::env::temp_dir().join(format!("disk-changed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("f"), b"before").unwrap();
    let (tree, _) = Node::scan(&root).unwrap();
    let image = Image::new(tree, Options::new("x", WHEN)).unwrap();
    std::fs::write(root.join("f"), b"after, longer").unwrap();
    let err = image.write(&mut Vec::new()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    std::fs::remove_dir_all(root).unwrap();
}
