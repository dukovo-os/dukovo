// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The names a directory keeps from its lookups (ADR 0659): found again
//! without the transaction, and never once a change moved them.

use std::format;
use std::sync::mpsc;
use std::time::Duration;

use radix_block::DeviceRef;
use radix_vfs::mem::KArc;
use radix_vfs::{Error, FileSystem, Kind, NodeRef};

use super::write::{mkfs_empty, writable};
use super::{DIR, FILE};
use crate::fs::BtrfsFs;

/// An empty filesystem, mounted to write.
fn mounted() -> KArc<BtrfsFs> {
    let disk = writable(&mkfs_empty(&[]));
    let device: DeviceRef = disk;
    BtrfsFs::mount_writable(device).unwrap()
}

/// The inode number of what `name` in `dir` names, or why there is none.
fn id(dir: &NodeRef, name: &str) -> Result<u64, Error> {
    dir.lookup(name).map(|node| node.stat().id)
}

#[test]
fn a_name_found_before_is_found_again_while_the_transaction_is_held() {
    let fs = mounted();
    let root = fs.root();
    let dir = root.create("d", Kind::Directory, DIR).unwrap();
    let file = dir.create("f", Kind::File, FILE).unwrap();
    let found = dir.lookup("f").unwrap();
    assert_eq!(found.stat().id, file.stat().id);

    // With the transaction held, as a long write holds it, a lookup the
    // directory kept answers; one it did not waits for the walk.
    let held = fs.hold_transaction();
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let looker = scope.spawn(|| {
            tx.send(id(&dir, "f")).unwrap();
            tx.send(id(&root, "d")).unwrap();
        });
        let kept = rx.recv_timeout(Duration::from_secs(10));
        assert_eq!(kept, Ok(Ok(file.stat().id)), "a kept name waited for the transaction");
        drop(held);
        assert!(rx.recv().unwrap().is_ok());
        looker.join().unwrap();
    });
}

#[test]
fn a_renamed_or_removed_name_is_never_found_where_it_was() {
    let fs = mounted();
    let root = fs.root();
    let a = root.create("a", Kind::Directory, DIR).unwrap();
    let b = root.create("b", Kind::Directory, DIR).unwrap();
    let one = a.create("one", Kind::File, FILE).unwrap().stat().id;
    let two = a.create("two", Kind::File, FILE).unwrap().stat().id;
    assert_eq!(id(&a, "one"), Ok(one));
    assert_eq!(id(&a, "two"), Ok(two));

    // Within the directory, and over another name kept.
    a.rename("one", &*a, "uno").unwrap();
    assert_eq!(id(&a, "one"), Err(Error::NotFound));
    assert_eq!(id(&a, "uno"), Ok(one));
    a.rename("uno", &*a, "two").unwrap();
    assert_eq!(id(&a, "uno"), Err(Error::NotFound));
    assert_eq!(id(&a, "two"), Ok(one));

    // Across directories, and back.
    a.rename("two", &*b, "moved").unwrap();
    assert_eq!(id(&a, "two"), Err(Error::NotFound));
    assert_eq!(id(&b, "moved"), Ok(one));
    b.rename("moved", &*a, "two").unwrap();
    assert_eq!(id(&b, "moved"), Err(Error::NotFound));
    assert_eq!(id(&a, "two"), Ok(one));

    // Removed while its node is held, then made again: a new file.
    let held = a.lookup("two").unwrap();
    a.remove("two").unwrap();
    assert_eq!(id(&a, "two"), Err(Error::NotFound));
    let again = a.create("two", Kind::File, FILE).unwrap().stat().id;
    assert_ne!(again, one);
    assert_eq!(id(&a, "two"), Ok(again));
    drop(held);

    // A second name, and the first one gone.
    let file = a.lookup("two").unwrap();
    b.link("second", &file).unwrap();
    assert_eq!(id(&b, "second"), Ok(again));
    a.remove("two").unwrap();
    assert_eq!(id(&a, "two"), Err(Error::NotFound));
    assert_eq!(id(&b, "second"), Ok(again));
}

#[test]
fn a_name_whose_node_went_is_looked_up_again() {
    let fs = mounted();
    let root = fs.root();
    let dir = root.create("d", Kind::Directory, DIR).unwrap();
    let made = dir.create("f", Kind::File, FILE).unwrap().stat().id;
    // Nobody holds the file once the lookup's node is dropped.
    assert_eq!(id(&dir, "f"), Ok(made));
    assert_eq!(id(&dir, "f"), Ok(made));
    fs.sync().unwrap();
    assert_eq!(id(&dir, "f"), Ok(made));
}

#[test]
fn more_names_than_a_directory_keeps_are_all_found() {
    let fs = mounted();
    let root = fs.root();
    let dir = root.create("d", Kind::Directory, DIR).unwrap();
    let files: std::vec::Vec<NodeRef> =
        (0..200).map(|n| dir.create(&format!("file-{n}"), Kind::File, FILE).unwrap()).collect();
    for _ in 0..3 {
        for (n, file) in files.iter().enumerate() {
            assert_eq!(id(&dir, &format!("file-{n}")), Ok(file.stat().id));
        }
    }
}

#[test]
fn lookups_while_another_cpu_changes_the_directory_find_what_is_there() {
    let fs = mounted();
    let root = fs.root();
    let dir = root.create("d", Kind::Directory, DIR).unwrap();
    let stays = dir.create("stays", Kind::File, FILE).unwrap().stat().id;
    let comes = dir.create("comes", Kind::File, FILE).unwrap().stat().id;
    std::thread::scope(|scope| {
        let changer = scope.spawn(|| {
            for n in 0..300 {
                dir.create(&format!("new-{n}"), Kind::File, FILE).unwrap();
                dir.rename("comes", &*dir, "goes").unwrap();
                dir.rename("goes", &*dir, "comes").unwrap();
                dir.remove(&format!("new-{n}")).unwrap();
            }
        });
        while !changer.is_finished() {
            assert_eq!(id(&dir, "stays"), Ok(stays));
            // Either name, never anything else.
            match id(&dir, "comes") {
                Ok(found) => assert_eq!(found, comes),
                Err(why) => assert_eq!(why, Error::NotFound),
            }
        }
        changer.join().unwrap();
    });
    assert_eq!(id(&dir, "comes"), Ok(comes));
    assert_eq!(id(&dir, "goes"), Err(Error::NotFound));
}
