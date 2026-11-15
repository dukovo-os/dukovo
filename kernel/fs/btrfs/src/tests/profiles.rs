// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Host tests of the profiles a filesystem's chunks use, which the kernel
//! tells in `/kernel/fs/btrfs/UUID/profiles` (radix-sysfs(5)): filesystems
//! `mkfs.btrfs` makes in several profiles, and one converted by a
//! balance. A conversion caught half done cannot be made on the host, as
//! a balance runs to its end, so that is tested from the chunks' kinds.

use std::format;
use std::string::String;

use radix_vfs::{Balance, BalanceArgs, FileSystem};

use super::raid::{disks_of, mkfs_disks, mount_all};
use crate::BtrfsFs;
use crate::chunk::{Profiles, kind};

/// The text of the file `profiles` for `profiles`.
fn text_of(profiles: Profiles) -> String {
    let mut text = String::new();
    profiles.write(&mut text).unwrap();
    text
}

/// What `fs` tells of its profiles.
fn told(fs: &BtrfsFs) -> String {
    text_of(fs.profiles())
}

#[test]
fn a_filesystem_of_one_disk_has_single_data_and_dup_metadata() {
    let disks = disks_of(&mkfs_disks(1, &["-d", "single", "-m", "dup"]));
    let fs = mount_all(&disks, false, false).unwrap();
    assert_eq!(told(&fs), "data single\nmetadata dup\nsystem dup\n");
}

#[test]
fn a_mirror_has_raid1_in_every_kind() {
    let disks = disks_of(&mkfs_disks(2, &["-d", "raid1", "-m", "raid1"]));
    let fs = mount_all(&disks, false, false).unwrap();
    assert_eq!(told(&fs), "data raid1\nmetadata raid1\nsystem raid1\n");
}

#[test]
fn the_kinds_may_differ() {
    let disks = disks_of(&mkfs_disks(2, &["-d", "raid0", "-m", "raid1"]));
    let fs = mount_all(&disks, false, false).unwrap();
    assert_eq!(told(&fs), "data raid0\nmetadata raid1\nsystem raid1\n");
}

#[test]
fn a_conversion_by_a_balance_shows_the_kinds_it_has_reached() {
    let mut disks = disks_of(&mkfs_disks(1, &["-d", "single", "-m", "dup"]));
    let fs = mount_all(&disks, true, false).unwrap();
    let second = disks_of(&mkfs_disks(1, &[]));
    fs.add_disk(second[0].clone()).unwrap();
    disks.push(second[0].clone());
    // Data first: the other kinds keep theirs.
    let data = BalanceArgs { on: true, convert: kind::RAID1, soft: false };
    fs.balance(&Balance { data, ..Balance::default() }).unwrap();
    assert_eq!(told(&fs), "data raid1\nmetadata dup\nsystem dup\n");
    let all = Balance { data, metadata: data, system: data };
    fs.balance(&all).unwrap();
    assert_eq!(told(&fs), "data raid1\nmetadata raid1\nsystem raid1\n");
}

#[test]
fn a_kind_with_two_profiles_lists_both_and_one_with_none_is_left_out() {
    // A balance that is part way: some data chunks moved to RAID1, some
    // not yet; the system chunk is DUP; no metadata chunk is there.
    let profiles = Profiles::of([
        kind::DATA,
        kind::DATA | kind::RAID1,
        kind::DATA | kind::RAID1,
        kind::SYSTEM | kind::DUP,
    ]);
    assert_eq!(text_of(profiles), "data single raid1\nsystem dup\n");
    assert_eq!(text_of(Profiles::of([])), "");
}

#[test]
fn the_words_come_in_one_order_whatever_the_order_of_the_chunks() {
    let one = Profiles::of([kind::DATA | kind::RAID1C3, kind::DATA | kind::RAID0, kind::DATA]);
    let other = Profiles::of([kind::DATA, kind::DATA | kind::RAID0, kind::DATA | kind::RAID1C3]);
    assert_eq!(one, other);
    assert_eq!(text_of(one), "data single raid0 raid1c3\n");
}

#[test]
fn every_profile_has_its_name() {
    for (profile, name) in [
        (0, "single"),
        (kind::RAID0, "raid0"),
        (kind::RAID1, "raid1"),
        (kind::DUP, "dup"),
        (kind::RAID10, "raid10"),
        (kind::RAID5, "raid5"),
        (kind::RAID6, "raid6"),
        (kind::RAID1C3, "raid1c3"),
        (kind::RAID1C4, "raid1c4"),
    ] {
        assert_eq!(text_of(Profiles::of([kind::METADATA | profile])), format!("metadata {name}\n"));
    }
}

#[test]
fn a_chunk_of_data_and_metadata_counts_for_both() {
    // A mixed filesystem's block groups.
    let profiles =
        Profiles::of([kind::DATA | kind::METADATA | kind::DUP, kind::SYSTEM | kind::DUP]);
    assert_eq!(text_of(profiles), "data dup\nmetadata dup\nsystem dup\n");
}
