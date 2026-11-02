// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Checking a filesystem without changing it (Phase 14a, ADR 0305): what
//! `fsck-btrfs` reports, as Linux's `btrfs check --readonly` checks, in
//! its order (feature `check`). It reads the trees from the disk, never
//! a mount's, and writes nothing.
//!
//! 1. **Root items**: every tree the root tree names, and the chunk tree,
//!    is walked from its root block down, each block read and checked as
//!    a read checks one (its checksum, header and keys, [`read_block`]),
//!    and each of its other copies too.
//! 2. **Extents**: each tree block and data extent the walk reaches has
//!    an extent item, which counts the references the walk found and
//!    lists the same back references (by tree, or by the block above it
//!    once that is shared, [`crate::refs`]); no extent item is left that
//!    nothing refers to; each block group's item counts the bytes of the
//!    extents in it, and the superblock all of them; each chunk lies on
//!    the device extents the device tree has for it, and each device's
//!    item counts them.
//! 3. **The free-space tree** has free exactly what no extent takes, but
//!    for the room kept around the superblock's copies, which it may have
//!    either way.
//! 4. **The subvolumes**: each inode has its item; a file has as many
//!    links as names; a directory's entries are each kept twice, by name
//!    and by order, and named back by what they name, its size is their
//!    names' length twice; a file's extents do not overlap, and its count
//!    of bytes is theirs.
//! 5. **Checksums**: every data extent of a file with checksums has them,
//!    and there are none where no data is; with `data`, the data of
//!    every copy is read and checked against them.
//! 6. **Root references**: each subvolume's name in its parent is named
//!    back by the root tree, both ways.
//!
//! Quota groups are not checked: Linux counts them again at a mount that
//! finds them inconsistent.

use std::collections::{BTreeMap, BTreeSet};
use std::format;
use std::string::String;
use std::vec::Vec;

use radix_vfs::{Error, Result};

use crate::chunk::kind;
use crate::csum::CSUM_SIZE;
use crate::format::{Key, RootItem, item, name_hash, tree, u16_at, u64_at};
use crate::inode::{Inode, flags};
use crate::refs::{Backref, FULL_BACKREF, backrefs, refs_flags};
use crate::superblock::{self, compat_ro};
use crate::tree::{Bounds, Cursor, Root, check_copy, read_block};
use crate::volume::Volume;

/// The object ID of the device items in the chunk tree.
const DEV_ITEMS: u64 = 1;

/// A data extent item's flag (`BTRFS_EXTENT_FLAG_DATA`), and a tree
/// block's.
const EXTENT_DATA: u64 = 1 << 0;
const EXTENT_TREE_BLOCK: u64 = 1 << 1;

/// The trees that hold files: the top subvolume, and the data relocation
/// tree.
const DATA_RELOC: u64 = (-9i64) as u64;

/// Past the last subvolume's ID.
const LAST_FREE: u64 = (-256i64) as u64;

/// The orphan items' object ID and type.
const ORPHAN: u64 = (-5i64) as u64;
const ORPHAN_ITEM: u8 = 48;

/// Where the superblock's copies are.
const SUPERS: [u64; 3] = [superblock::OFFSET, 64 << 20, 256 << 30];

/// The phases, as `btrfs check` names them.
pub const PHASES: [&str; 7] = [
    "checking root items",
    "checking extents",
    "checking free space tree",
    "checking fs roots",
    "checking csums",
    "checking root refs",
    "checking quota groups skipped (not checked by Dukovo)",
];

/// What a check found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// What is wrong, a sentence each.
    pub problems: Vec<String>,
    /// The tree blocks the walk reached.
    pub tree_blocks: u64,
    /// The data extents files refer to.
    pub data_extents: u64,
    /// The bytes every extent takes.
    pub bytes_used: u64,
    /// The subvolumes, and the directories and other files in them.
    pub subvolumes: u64,
    /// Directories.
    pub directories: u64,
    /// Files that are not directories.
    pub files: u64,
    /// Whether there is a log of fsyncs a mount replays.
    pub log: bool,
}

/// An extent item: its references and flags, its level for a tree
/// block, its length, and its back references with their counts.
struct ExtentItem {
    refs: u64,
    flags: u64,
    length: u64,
    tree_block: bool,
    level: u64,
    backrefs: BTreeMap<Ref, u64>,
}

/// A back reference, as the walk finds it and the extent tree keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Ref {
    Tree(u64),
    Shared(u64),
    Data { root: u64, ino: u64, offset: u64 },
    SharedData(u64),
}

impl From<Backref> for Ref {
    fn from(r: Backref) -> Ref {
        match r {
            Backref::Tree(id) => Ref::Tree(id),
            Backref::Shared(at) => Ref::Shared(at),
            Backref::Data { root, ino, offset } => Ref::Data { root, ino, offset },
            Backref::SharedData(at) => Ref::SharedData(at),
        }
    }
}

/// A tree block the walk reached: its level and owner, and who refers to
/// it.
struct Reached {
    level: u8,
    owner: u64,
    refs: BTreeMap<Ref, u64>,
}

/// A data extent files refer to: its length on the disk, who refers to
/// it, and whether all of it is to have checksums.
#[derive(Default)]
struct DataRefs {
    length: u64,
    refs: BTreeMap<Ref, u64>,
    sums: bool,
}

/// The state of a check.
struct Check<'v> {
    volume: &'v Volume,
    report: Report,
    extents: BTreeMap<u64, ExtentItem>,
    blocks: BTreeMap<u64, Reached>,
    data: BTreeMap<u64, DataRefs>,
    /// Each subvolume tree to check, by its ID, and its root block.
    subvolumes: Vec<(u64, Root)>,
}

impl Check<'_> {
    fn problem(&mut self, text: String) {
        self.report.problems.push(text);
    }
}

/// Checks the filesystem `volume` holds, as the module says, calling
/// `phase` with the number and name of each phase as it starts; reads and
/// checks every copy of the data too if `data`.
///
/// Fails only when the check cannot go on: as reading the root tree or
/// the extent tree fails.
pub fn check(volume: &Volume, data: bool, phase: &mut dyn FnMut(usize, &str)) -> Result<Report> {
    let mut c = Check {
        volume,
        report: Report::default(),
        extents: BTreeMap::new(),
        blocks: BTreeMap::new(),
        data: BTreeMap::new(),
        subvolumes: Vec::new(),
    };
    c.report.log = volume.superblock().log_root != 0;
    c.superblocks();
    phase(1, PHASES[0]);
    c.read_extent_items()?;
    c.walk_all()?;
    phase(2, PHASES[1]);
    c.extents();
    c.chunks()?;
    phase(3, PHASES[2]);
    c.free_space()?;
    phase(4, PHASES[3]);
    let subvolumes = core::mem::take(&mut c.subvolumes);
    for &(id, root) in &subvolumes {
        c.files(id, &root)?;
    }
    phase(5, PHASES[4]);
    c.sums(data)?;
    phase(6, PHASES[5]);
    c.root_refs(&subvolumes)?;
    phase(7, PHASES[6]);
    c.report.subvolumes = subvolumes.iter().filter(|s| s.0 != DATA_RELOC).count() as u64;
    Ok(c.report)
}

/// A chunk as the check reads it: its start, length, kind, bytes a
/// stripe holds, and its stripes' devices and offsets.
type ChunkRow = (u64, u64, u64, u64, Vec<(u64, u64)>);

impl Check<'_> {
    /// The superblock's copies on each disk: each that the disk reaches is
    /// a Btrfs superblock of this filesystem with its checksum.
    fn superblocks(&mut self) {
        let sb = self.volume.superblock();
        for (index, disk) in self.volume.disks().iter().enumerate() {
            let Some(cache) = disk.cache() else { continue };
            for at in SUPERS {
                if at + superblock::SIZE as u64 > disk.bytes() {
                    continue;
                }
                let mut bytes = std::vec![0; superblock::SIZE];
                // A copy says where it is, and is checked as the first is.
                let good = cache.read(at, &mut bytes).is_ok()
                    && u64_at(&bytes, 0x40) == superblock::MAGIC
                    && u64_at(&bytes, 0x30) == at
                    && bytes[0x20..0x30] == sb.fsid
                    && sb.csum_type.matches(&bytes[CSUM_SIZE..], &bytes[..CSUM_SIZE]);
                if !good {
                    self.problem(format!("disk {index}: the superblock's copy at {at} is bad"));
                }
            }
        }
    }

    /// Reads every extent item, with its back references.
    fn read_extent_items(&mut self) -> Result<()> {
        let volume = self.volume;
        let root = volume.tree(tree::EXTENT)?.ok_or(Error::Corrupt)?;
        let nodesize = u64::from(volume.superblock().nodesize);
        let mut cursor = Cursor::seek(volume, &root, &Key::new(0, 0, 0))?;
        while let Some((key, data)) = cursor.get() {
            if matches!(key.kind, item::EXTENT_ITEM | item::METADATA_ITEM) {
                match refs_flags(data).and_then(|rf| Ok((rf, backrefs(volume, &key, data)?))) {
                    Ok(((refs, flags), list)) => {
                        let skinny = key.kind == item::METADATA_ITEM;
                        let tree_block = skinny || flags & EXTENT_TREE_BLOCK != 0;
                        // A tree block's item that is not skinny has its
                        // first key and level after the header.
                        let level = if skinny {
                            key.offset
                        } else if tree_block {
                            u64::from(data.get(24 + Key::SIZE).copied().unwrap_or(0))
                        } else {
                            0
                        };
                        let mut map = BTreeMap::new();
                        for (r, count) in list {
                            *map.entry(Ref::from(r)).or_insert(0) += u64::from(count);
                        }
                        let length = if skinny { nodesize } else { key.offset };
                        let extent =
                            ExtentItem { refs, flags, length, tree_block, level, backrefs: map };
                        if tree_block == (flags & EXTENT_DATA != 0) && !skinny {
                            self.problem(format!(
                                "extent {}: its flags are {flags:#x}",
                                key.objectid
                            ));
                        }
                        if self.extents.insert(key.objectid, extent).is_some() {
                            self.problem(format!("extent {}: it has two items", key.objectid));
                        }
                    }
                    Err(e) => self
                        .problem(format!("extent {}: its item cannot be read: {e}", key.objectid)),
                }
            }
            cursor.advance()?;
        }
        Ok(())
    }

    /// Walks every tree: the chunk tree, the root tree, and each tree the
    /// root tree names.
    fn walk_all(&mut self) -> Result<()> {
        let volume = self.volume;
        let sb = volume.superblock();
        let chunk = volume.chunk_root();
        self.walk(tree::CHUNK, chunk);
        let root = volume.root_tree();
        self.walk(tree::ROOT, root);
        let mut cursor = Cursor::seek(volume, &root, &Key::new(0, 0, 0))?;
        let mut trees = Vec::new();
        while let Some((key, data)) = cursor.get() {
            if key.kind == item::ROOT_ITEM {
                match RootItem::read(data) {
                    Some(r) if r.generation <= sb.generation => {
                        let root =
                            Root { bytenr: r.bytenr, level: r.level, generation: r.generation };
                        trees.push((key.objectid, root, r.refs));
                    }
                    Some(r) => self.problem(format!(
                        "tree {}: its root item says transaction {}, past the last ({})",
                        key.objectid as i64, r.generation, sb.generation
                    )),
                    None => self.problem(format!(
                        "tree {}: its root item is too short",
                        key.objectid as i64
                    )),
                }
            }
            cursor.advance()?;
        }
        for (id, root, refs) in trees {
            self.walk(id, root);
            let files =
                id == tree::FS || id == DATA_RELOC || (tree::FIRST_FREE..LAST_FREE).contains(&id);
            if files && refs > 0 {
                self.subvolumes.push((id, root));
            }
        }
        Ok(())
    }

    /// Walks tree `id` from its root block `top`, noting who refers to
    /// each block and data extent; a block reached before is not walked
    /// again.
    fn walk(&mut self, id: u64, top: Root) {
        let volume = self.volume;
        let mut stack = std::vec![(top, Bounds::default(), Ref::Tree(id))];
        while let Some((want, bounds, by)) = stack.pop() {
            if let Some(reached) = self.blocks.get_mut(&want.bytenr) {
                *reached.refs.entry(by).or_insert(0) += 1;
                continue;
            }
            let block = match read_block(volume, &want, &bounds) {
                Ok(block) => block,
                Err(e) => {
                    self.problem(format!(
                        "tree {}: block {} cannot be read: {e}",
                        id as i64, want.bytenr
                    ));
                    continue;
                }
            };
            self.other_copies(&want);
            let mut refs = BTreeMap::new();
            refs.insert(by, 1);
            self.blocks
                .insert(want.bytenr, Reached { level: block.level(), owner: block.owner(), refs });
            // Who the block refers to: by its tree, or by its address once
            // it is marked so.
            let full = self.extents.get(&want.bytenr).is_some_and(|e| e.flags & FULL_BACKREF != 0);
            if !block.is_leaf() {
                for slot in 0..block.nritems() {
                    let by = if full { Ref::Shared(want.bytenr) } else { Ref::Tree(block.owner()) };
                    stack.push((block.child(slot), Bounds::of_child(&block, slot), by));
                }
                continue;
            }
            for slot in 0..block.nritems() {
                let key = block.key(slot);
                let d = block.item(slot);
                // A regular or preallocated extent: its address, length on
                // the disk, and offset into it.
                if key.kind != item::EXTENT_DATA || d.len() != 53 || !matches!(d[20], 1 | 2) {
                    continue;
                }
                let (disk_bytenr, disk_len, offset) = (u64_at(d, 21), u64_at(d, 29), u64_at(d, 37));
                if disk_bytenr == 0 {
                    continue;
                }
                let by = if full {
                    Ref::SharedData(want.bytenr)
                } else {
                    Ref::Data {
                        root: block.owner(),
                        ino: key.objectid,
                        offset: key.offset.wrapping_sub(offset),
                    }
                };
                let extent = self.data.entry(disk_bytenr).or_default();
                if extent.length != 0 && extent.length != disk_len {
                    let text = format!(
                        "data extent {disk_bytenr}: files say it is {} and {disk_len} bytes",
                        extent.length
                    );
                    self.report.problems.push(text);
                }
                extent.length = disk_len;
                *extent.refs.entry(by).or_insert(0) += 1;
            }
        }
    }

    /// Reads the copies of tree block `want` past the first good one, and
    /// notes each that is bad.
    fn other_copies(&mut self, want: &Root) {
        let volume = self.volume;
        let nodesize = volume.superblock().nodesize as usize;
        let Ok(place) = volume.place(want.bytenr) else { return };
        for (n, site) in place.copies().iter().enumerate() {
            let mut bytes = std::vec![0; nodesize];
            let what = match volume.read_site(*site, &mut bytes) {
                Err(e) => Some(format!("cannot be read: {e}")),
                Ok(()) => match check_copy(volume, want, bytes).0 {
                    Some(crate::tree::Fault::Sum) => Some(String::from("has a bad checksum")),
                    Some(crate::tree::Fault::Header) => Some(String::from("has a bad header")),
                    None => None,
                },
            };
            if let Some(what) = what {
                self.problem(format!(
                    "tree block {}: copy {} (disk {}, byte {}) {what}",
                    want.bytenr,
                    n + 1,
                    site.disk,
                    site.at
                ));
            }
        }
    }

    /// Phase 2: the extent items against what the walk found, and the
    /// block groups and the superblock against the extents.
    fn extents(&mut self) {
        let mut problems = Vec::new();
        for (&bytenr, reached) in &self.blocks {
            let found: u64 = reached.refs.values().sum();
            match self.extents.get(&bytenr) {
                None => problems.push(format!(
                    "tree block {bytenr} of tree {} has no extent item",
                    reached.owner as i64
                )),
                Some(e) => {
                    if !e.tree_block || e.level != u64::from(reached.level) {
                        problems.push(format!(
                            "tree block {bytenr}: its extent item says level {}, the block {}",
                            e.level, reached.level
                        ));
                    }
                    if e.refs != found || e.backrefs != reached.refs {
                        problems.push(format!(
                            "tree block {bytenr}: {found} references found ({:?}), its extent item counts {} ({:?})",
                            reached.refs, e.refs, e.backrefs
                        ));
                    }
                }
            }
        }
        for (&bytenr, d) in &self.data {
            let found: u64 = d.refs.values().sum();
            match self.extents.get(&bytenr) {
                None => problems.push(format!("data extent {bytenr} has no extent item")),
                Some(e) => {
                    if e.tree_block || e.length != d.length {
                        problems.push(format!(
                            "data extent {bytenr}: its item says {} bytes, its files {}",
                            e.length, d.length
                        ));
                    }
                    if e.refs != found || e.backrefs != d.refs {
                        problems.push(format!(
                            "data extent {bytenr}: {found} references found ({:?}), its extent item counts {} ({:?})",
                            d.refs, e.refs, e.backrefs
                        ));
                    }
                }
            }
        }
        for (&bytenr, e) in &self.extents {
            let reached = if e.tree_block {
                self.blocks.contains_key(&bytenr)
            } else {
                self.data.contains_key(&bytenr)
            };
            if !reached {
                problems.push(format!("extent {bytenr} has an item, but nothing refers to it"));
            }
        }
        self.report.tree_blocks = self.blocks.len() as u64;
        self.report.data_extents = self.data.len() as u64;
        self.report.bytes_used = self.extents.values().map(|e| e.length).sum();
        let sb = self.volume.superblock();
        if sb.bytes_used != self.report.bytes_used {
            problems.push(format!(
                "the superblock says {} bytes are used, the extents take {}",
                sb.bytes_used, self.report.bytes_used
            ));
        }
        self.report.problems.extend(problems);
    }

    /// The block groups, by their start: their length, flags and used
    /// bytes.
    fn groups(&self) -> Result<BTreeMap<u64, (u64, u64, u64)>> {
        let volume = self.volume;
        let group_tree = if volume.superblock().compat_ro_flags & compat_ro::BLOCK_GROUP_TREE != 0 {
            tree::BLOCK_GROUP
        } else {
            tree::EXTENT
        };
        let root = volume.tree(group_tree)?.ok_or(Error::Corrupt)?;
        let mut groups = BTreeMap::new();
        let mut cursor = Cursor::seek(volume, &root, &Key::new(0, 0, 0))?;
        while let Some((key, data)) = cursor.get() {
            if key.kind == item::BLOCK_GROUP_ITEM && data.len() >= 24 {
                groups.insert(key.objectid, (key.offset, u64_at(data, 16), u64_at(data, 0)));
            }
            cursor.advance()?;
        }
        Ok(groups)
    }

    /// The chunks against the block groups and the device tree, and the
    /// device items against the device extents; each extent in a group
    /// of its kind, and each group's count of bytes.
    fn chunks(&mut self) -> Result<()> {
        let volume = self.volume;
        let groups = self.groups()?;
        let chunks: Vec<ChunkRow> = volume.with_chunks(|map| {
            map.chunks()
                .iter()
                .map(|c| {
                    let stripes = c.stripes.iter().map(|s| (s.devid, s.offset)).collect();
                    (c.start, c.length, c.kind, c.stripe_bytes(), stripes)
                })
                .collect()
        });
        let mut problems = Vec::new();
        const TYPE: u64 = kind::DATA | kind::SYSTEM | kind::METADATA;
        for &(start, length, flags, _, _) in &chunks {
            match groups.get(&start) {
                Some(&(len, f, _)) if len == length && f == flags => {}
                Some(_) => problems.push(format!("chunk {start}: its block group differs from it")),
                None => problems.push(format!("chunk {start} has no block group")),
            }
        }
        for (&start, &(length, flags, used)) in &groups {
            if !chunks.iter().any(|c| c.0 == start) {
                problems.push(format!("block group {start} has no chunk"));
            }
            let end = start + length;
            let mut counted = 0;
            for (&at, e) in self.extents.range(start..end) {
                counted += e.length;
                let fits = if e.tree_block {
                    flags & (kind::METADATA | kind::SYSTEM) != 0
                } else {
                    flags & kind::DATA != 0
                };
                if !fits || at + e.length > end {
                    problems.push(format!(
                        "extent {at} is not where it belongs: in block group {start} of {:#x}",
                        flags & TYPE
                    ));
                }
            }
            if counted != used {
                problems.push(format!(
                    "block group {start}: its item counts {used} bytes used, its extents take {counted}"
                ));
            }
        }
        // The device tree's extents, by device and where they start.
        let dev = volume.tree(tree::DEV)?.ok_or(Error::Corrupt)?;
        let mut extents: BTreeMap<(u64, u64), (u64, u64)> = BTreeMap::new();
        let mut cursor = Cursor::seek(volume, &dev, &Key::new(0, 0, 0))?;
        while let Some((key, data)) = cursor.get() {
            if key.kind == item::DEV_EXTENT && data.len() >= 32 {
                extents.insert((key.objectid, key.offset), (u64_at(data, 16), u64_at(data, 24)));
            }
            cursor.advance()?;
        }
        let mut matched = BTreeSet::new();
        for (start, _, _, stripe, stripes) in &chunks {
            for &(devid, offset) in stripes {
                match extents.get(&(devid, offset)) {
                    Some(&(chunk, len)) if chunk == *start && len == *stripe => {
                        matched.insert((devid, offset));
                    }
                    _ => problems.push(format!(
                        "chunk {start}: device {devid} has no extent for its stripe at {offset}"
                    )),
                }
            }
        }
        let mut last_end: BTreeMap<u64, u64> = BTreeMap::new();
        let mut used: BTreeMap<u64, u64> = BTreeMap::new();
        for (&(devid, offset), &(_, len)) in &extents {
            if !matched.contains(&(devid, offset)) {
                problems.push(format!("device {devid}: its extent at {offset} has no chunk"));
            }
            let end = last_end.entry(devid).or_insert(0);
            if offset < *end {
                problems
                    .push(format!("device {devid}: its extents at {offset} and before overlap"));
            }
            *end = (*end).max(offset + len);
            *used.entry(devid).or_insert(0) += len;
        }
        // The device items, in the chunk tree.
        let chunk_root = volume.chunk_root();
        let mut cursor = Cursor::seek(volume, &chunk_root, &Key::first(DEV_ITEMS, item::DEV_ITEM))?;
        while let Some((key, data)) = cursor.get() {
            if !key.is(DEV_ITEMS, item::DEV_ITEM) {
                break;
            }
            let (total, bytes_used) = (u64_at(data, 8), u64_at(data, 16));
            let counted = used.get(&key.offset).copied().unwrap_or(0);
            if bytes_used != counted {
                problems.push(format!(
                    "device {}: its item counts {bytes_used} bytes in chunks, its extents take {counted}",
                    key.offset
                ));
            }
            if last_end.get(&key.offset).is_some_and(|&end| end > total) {
                problems.push(format!("device {}: its extents go past its end", key.offset));
            }
            cursor.advance()?;
        }
        self.report.problems.extend(problems);
        Ok(())
    }

    /// Phase 3: each block group's free space, as the free-space tree has
    /// it, against its extents.
    fn free_space(&mut self) -> Result<()> {
        let volume = self.volume;
        if volume.superblock().compat_ro_flags & compat_ro::FREE_SPACE_TREE == 0 {
            return Ok(());
        }
        let Some(fst) = volume.tree(tree::FREE_SPACE)? else {
            self.problem(String::from(
                "the superblock says there is a free-space tree, but there is none",
            ));
            return Ok(());
        };
        let mut problems = Vec::new();
        for (start, (length, _, _)) in self.groups()? {
            let end = start + length;
            let free = match crate::space::read_free(volume, &fst, start, length) {
                Ok((free, _)) => free,
                Err(e) => {
                    problems
                        .push(format!("block group {start}: its free space cannot be read: {e}"));
                    continue;
                }
            };
            let mut mirrors = crate::space::NO_MIRRORS;
            let n = crate::space::mirror_ranges(volume, start, &mut mirrors)?;
            let mirrors = &mirrors[..n];
            // What is neither free nor taken: walk both in order.
            let mut taken: Vec<(u64, u64)> =
                self.extents.range(start..end).map(|(&at, e)| (at, at + e.length)).collect();
            taken.extend(mirrors.iter().copied());
            taken.sort_unstable();
            for &(lo, hi) in &free {
                if lo < start || hi > end {
                    problems
                        .push(format!("block group {start}: free space {lo}..{hi} is outside it"));
                }
                for &(a, b) in taken.iter().filter(|r| r.0 < hi && lo < r.1) {
                    if !mirrors.contains(&(a, b)) {
                        problems.push(format!(
                            "block group {start}: {lo}..{hi} is free, but an extent takes {a}..{b}"
                        ));
                    }
                }
            }
            let mut at = start;
            let mut covered = free.iter().copied().chain(taken.iter().copied()).collect::<Vec<_>>();
            covered.sort_unstable();
            for (lo, hi) in covered {
                if lo > at {
                    problems
                        .push(format!("block group {start}: {at}..{lo} is neither free nor taken"));
                }
                at = at.max(hi);
            }
            if at < end {
                problems
                    .push(format!("block group {start}: {at}..{end} is neither free nor taken"));
            }
        }
        self.report.problems.extend(problems);
        Ok(())
    }
}

/// A name of an inode in a directory: the directory, the entry's index,
/// and the name.
type Name = (u64, u64, Vec<u8>);

/// What a subvolume keeps of one inode.
#[derive(Default)]
struct InodeItems {
    inode: Option<Inode>,
    /// Its names, from its `INODE_REF` and `INODE_EXTREF` items.
    names: Vec<Name>,
    /// A directory's entries by order: index, name, what each names
    /// (its key) and the type byte.
    index: BTreeMap<u64, (Vec<u8>, Key, u8)>,
    /// Its entries by name: name, what each names and the type byte.
    by_name: Vec<(Vec<u8>, Key, u8)>,
    /// Its file extents: offset, the bytes it covers, and the bytes it
    /// counts.
    extents: Vec<(u64, u64, u64)>,
    orphan: bool,
}

/// The entries of a directory item: what each names, its type byte and
/// its name.
fn dir_entries(data: &[u8]) -> Option<Vec<(Key, u8, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let location = Key::read(data, at);
        // After the key: the transaction, the lengths of the data and of
        // the name, and the type byte.
        let data_len = usize::from(u16_at(data, at + 25));
        let name_len = usize::from(u16_at(data, at + 27));
        let kind = *data.get(at + 29)?;
        let name = data.get(at + 30..at + 30 + name_len)?.to_vec();
        out.push((location, kind, name));
        at += 30 + name_len + data_len;
    }
    (at == data.len()).then_some(out)
}

/// The type byte of a directory entry for an inode of `mode`.
fn type_of(mode: u32) -> u8 {
    match mode & 0o170_000 {
        0o100_000 => 1,
        0o040_000 => 2,
        0o020_000 => 3,
        0o060_000 => 4,
        0o010_000 => 5,
        0o140_000 => 6,
        0o120_000 => 7,
        _ => 0,
    }
}

impl Check<'_> {
    /// Phase 4: the inodes and directories of subvolume `id`, whose tree's
    /// root block is `root`.
    fn files(&mut self, id: u64, root: &Root) -> Result<()> {
        let volume = self.volume;
        let mut inodes: BTreeMap<u64, InodeItems> = BTreeMap::new();
        let mut problems = Vec::new();
        let mut cursor = Cursor::seek(volume, root, &Key::new(0, 0, 0))?;
        let what = |ino: u64| format!("subvolume {} inode {ino}", id as i64);
        while let Some((key, data)) = cursor.get() {
            let ino = key.objectid;
            match key.kind {
                item::INODE_ITEM => match Inode::read(data) {
                    Ok(inode) => inodes.entry(ino).or_default().inode = Some(inode),
                    Err(_) => problems.push(format!("{}: its inode item is too short", what(ino))),
                },
                item::INODE_REF => {
                    let mut at = 0;
                    while at + 10 <= data.len() {
                        let (index, len) = (u64_at(data, at), usize::from(u16_at(data, at + 8)));
                        let name = data.get(at + 10..at + 10 + len).unwrap_or_default().to_vec();
                        inodes.entry(ino).or_default().names.push((key.offset, index, name));
                        at += 10 + len;
                    }
                }
                item::INODE_EXTREF => {
                    let mut at = 0;
                    while at + 18 <= data.len() {
                        let (dir, index) = (u64_at(data, at), u64_at(data, at + 8));
                        let len = usize::from(u16_at(data, at + 16));
                        let name = data.get(at + 18..at + 18 + len).unwrap_or_default().to_vec();
                        inodes.entry(ino).or_default().names.push((dir, index, name));
                        at += 18 + len;
                    }
                }
                item::DIR_ITEM | item::DIR_INDEX => match dir_entries(data) {
                    Some(entries) => {
                        let items = inodes.entry(ino).or_default();
                        for (location, kind, name) in entries {
                            if key.kind == item::DIR_INDEX {
                                items.index.insert(key.offset, (name, location, kind));
                            } else {
                                if name_hash(&name) != key.offset {
                                    problems.push(format!(
                                        "{}: the entry {} is under another name's hash",
                                        what(ino),
                                        String::from_utf8_lossy(&name)
                                    ));
                                }
                                items.by_name.push((name, location, kind));
                            }
                        }
                    }
                    None => problems.push(format!("{}: a directory item is cut short", what(ino))),
                },
                item::EXTENT_DATA => {
                    let counts = match data.get(20) {
                        // Inline: the bytes it holds, uncompressed.
                        Some(0) => Some((u64_at(data, 8), u64_at(data, 8))),
                        // Regular and preallocated: the bytes of the file
                        // it covers, counted unless it is a hole.
                        Some(1 | 2) if data.len() == 53 => {
                            let len = u64_at(data, 45);
                            Some((len, if u64_at(data, 21) == 0 { 0 } else { len }))
                        }
                        _ => None,
                    };
                    match counts {
                        Some((len, counted)) => {
                            inodes.entry(ino).or_default().extents.push((key.offset, len, counted));
                        }
                        None => problems.push(format!(
                            "{}: a file extent at {} is bad",
                            what(ino),
                            key.offset
                        )),
                    }
                }
                ORPHAN_ITEM if ino == ORPHAN => inodes.entry(key.offset).or_default().orphan = true,
                _ => {}
            }
            cursor.advance()?;
        }
        let top =
            if id == DATA_RELOC || id == tree::FS || id >= tree::FIRST_FREE { 256 } else { 0 };
        for (&ino, items) in &inodes {
            if ino == ORPHAN {
                continue;
            }
            let Some(inode) = items.inode else {
                problems.push(format!("{} has items but no inode item", what(ino)));
                continue;
            };
            let dir = inode.mode & 0o170_000 == 0o040_000;
            if dir {
                self.report.directories += 1;
            } else {
                self.report.files += 1;
            }
            // Its names, each in its directory both ways.
            for (parent, index, name) in &items.names {
                if *parent == ino && ino == top {
                    continue;
                }
                let names_it = |e: &(Vec<u8>, Key, u8)| {
                    e.0 == *name && e.1.objectid == ino && e.1.kind == item::INODE_ITEM
                };
                let Some(p) = inodes.get(parent) else {
                    problems.push(format!("{}: its directory {parent} is missing", what(ino)));
                    continue;
                };
                if !p.index.get(index).is_some_and(names_it) || !p.by_name.iter().any(names_it) {
                    problems.push(format!(
                        "{}: directory {parent} does not name it {} at {index}",
                        what(ino),
                        String::from_utf8_lossy(name)
                    ));
                }
            }
            let links = items.names.iter().filter(|n| !(n.0 == ino && ino == top)).count() as u32;
            let want = if dir { 1 } else { links };
            if inode.nlink != want && !(items.orphan && inode.nlink == 0) {
                problems.push(format!(
                    "{}: it has {} links, and {links} names",
                    what(ino),
                    inode.nlink
                ));
            }
            if dir {
                let names: u64 = items.index.values().map(|e| e.0.len() as u64).sum();
                if inode.size != 2 * names {
                    problems.push(format!(
                        "{}: its size is {}, and its names' length twice is {}",
                        what(ino),
                        inode.size,
                        2 * names
                    ));
                }
                if items.by_name.len() != items.index.len() {
                    problems.push(format!(
                        "{}: it has {} entries by name and {} by order",
                        what(ino),
                        items.by_name.len(),
                        items.index.len()
                    ));
                }
                // What each entry names names it back.
                for (index, (name, location, kind)) in &items.index {
                    if !items.by_name.iter().any(|e| e.0 == *name && e.1 == *location) {
                        problems.push(format!(
                            "{}: entry {} at {index} has no entry by name",
                            what(ino),
                            String::from_utf8_lossy(name)
                        ));
                    }
                    if location.kind != item::INODE_ITEM {
                        continue;
                    }
                    let target = inodes.get(&location.objectid);
                    let back = target.is_some_and(|t| {
                        t.names.iter().any(|n| n.0 == ino && n.1 == *index && n.2 == *name)
                    });
                    let typed =
                        target.and_then(|t| t.inode).is_some_and(|i| type_of(i.mode) == *kind);
                    if !back || !typed {
                        problems.push(format!(
                            "{}: entry {} names inode {}, which does not name it back as such",
                            what(ino),
                            String::from_utf8_lossy(name),
                            location.objectid
                        ));
                    }
                }
            }
            // Its extents: in order, apart, and counted.
            let mut end = 0;
            let mut counted = 0;
            for &(offset, len, count) in &items.extents {
                if offset < end {
                    problems
                        .push(format!("{}: its extents at {offset} and before overlap", what(ino)));
                }
                end = end.max(offset + len);
                counted += count;
            }
            if !dir && inode.nbytes != counted {
                problems.push(format!(
                    "{}: it counts {} bytes, and its extents {counted}",
                    what(ino),
                    inode.nbytes
                ));
            }
        }
        // Which data extents are to have checksums.
        let leaves = self.subvolume_sums(root, &inodes)?;
        for bytenr in leaves {
            if let Some(d) = self.data.get_mut(&bytenr) {
                d.sums = true;
            }
        }
        self.report.problems.extend(problems);
        Ok(())
    }

    /// The data extents the files of subvolume `id` with checksums refer
    /// to.
    fn subvolume_sums(&self, root: &Root, inodes: &BTreeMap<u64, InodeItems>) -> Result<Vec<u64>> {
        let volume = self.volume;
        let mut out = Vec::new();
        let mut cursor = Cursor::seek(volume, root, &Key::new(0, 0, 0))?;
        while let Some((key, data)) = cursor.get() {
            let regular = key.kind == item::EXTENT_DATA && data.len() == 53 && data[20] == 1;
            if regular && u64_at(data, 21) != 0 {
                let sums = inodes
                    .get(&key.objectid)
                    .and_then(|i| i.inode)
                    .is_some_and(|i| i.flags & flags::NODATASUM == 0);
                if sums {
                    out.push(u64_at(data, 21));
                }
            }
            cursor.advance()?;
        }
        Ok(out)
    }

    /// Phase 5: the checksum tree against the data extents, and with
    /// `data` the data of every copy against it.
    fn sums(&mut self, data: bool) -> Result<()> {
        let volume = self.volume;
        let sb = volume.superblock();
        let (size, sector) = (sb.csum_type.size(), u64::from(sb.sectorsize));
        let Some(csums) = volume.tree(tree::CSUM)? else {
            self.problem(String::from("there is no checksum tree"));
            return Ok(());
        };
        // The ranges the checksum tree covers.
        let mut covered: Vec<(u64, u64)> = Vec::new();
        let mut cursor =
            Cursor::seek(volume, &csums, &Key::first(tree::EXTENT_CSUM, item::EXTENT_CSUM))?;
        while let Some((key, sums)) = cursor.get() {
            if !key.is(tree::EXTENT_CSUM, item::EXTENT_CSUM) {
                break;
            }
            if sums.len() % size != 0 {
                self.problem(format!("the checksums at {} are cut short", key.offset));
            }
            let end = key.offset + (sums.len() / size) as u64 * sector;
            if covered.last().is_some_and(|r| r.1 > key.offset) {
                self.problem(format!("the checksums at {} overlap those before", key.offset));
            }
            covered.push((key.offset, end));
            cursor.advance()?;
        }
        let mut problems = Vec::new();
        // Each range, in order, against the data extents.
        for &(lo, hi) in &covered {
            let held: u64 = self
                .data
                .range(..hi)
                .filter(|(s, d)| **s + d.length > lo)
                .map(|(s, d)| (s + d.length).min(hi) - (*s).max(lo))
                .sum();
            if held != hi - lo {
                problems
                    .push(format!("checksums for {lo}..{hi}, where only {held} bytes are data"));
            }
        }
        for (&start, d) in &self.data {
            if !d.sums {
                continue;
            }
            let end = start + d.length;
            let have: u64 = covered
                .iter()
                .filter(|r| r.0 < end && start < r.1)
                .map(|r| r.1.min(end) - r.0.max(start))
                .sum();
            if have != d.length {
                problems.push(format!(
                    "data extent {start}: {} of its {} bytes have checksums",
                    have, d.length
                ));
            } else if data {
                self.verify(&csums, start, d.length, &mut problems)?;
            }
        }
        self.report.problems.extend(problems);
        Ok(())
    }

    /// Reads every copy of the data extent at `start`, `length` bytes,
    /// and checks each sector against the checksum tree `csums`.
    fn verify(
        &self,
        csums: &Root,
        start: u64,
        length: u64,
        problems: &mut Vec<String>,
    ) -> Result<()> {
        let volume = self.volume;
        let sb = volume.superblock();
        let sector = u64::from(sb.sectorsize);
        let mut at = start;
        while at < start + length {
            let place = volume.place(at)?;
            let len = place.len.min(start + length - at).min(1 << 20);
            let count = (len / sector) as usize;
            let mut sums = Vec::with_capacity(count);
            crate::extent::each_sum(volume, csums, at, count, &mut |_, sum| {
                sums.push(sum.map(<[u8]>::to_vec));
            })?;
            for (n, site) in place.copies().iter().enumerate() {
                let mut bytes = std::vec![0; len as usize];
                if let Err(e) = volume.read_site(*site, &mut bytes) {
                    problems.push(format!("data at {at}: copy {} cannot be read: {e}", n + 1));
                    continue;
                }
                for (i, piece) in bytes.chunks(sector as usize).enumerate() {
                    let good = sums
                        .get(i)
                        .and_then(Option::as_ref)
                        .is_some_and(|s| sb.csum_type.matches(piece, s));
                    if !good {
                        problems.push(format!(
                            "data at {}: copy {} has a bad checksum",
                            at + i as u64 * sector,
                            n + 1
                        ));
                    }
                }
            }
            at += len;
        }
        Ok(())
    }

    /// Phase 6: each subvolume's entry in its parent against the root
    /// tree's references, both ways.
    fn root_refs(&mut self, subvolumes: &[(u64, Root)]) -> Result<()> {
        let volume = self.volume;
        let root = volume.root_tree();
        let mut refs: BTreeMap<(u64, u64), (u64, u64, Vec<u8>)> = BTreeMap::new();
        let mut backs: BTreeMap<(u64, u64), (u64, u64, Vec<u8>)> = BTreeMap::new();
        let mut cursor = Cursor::seek(volume, &root, &Key::new(0, 0, 0))?;
        while let Some((key, data)) = cursor.get() {
            if matches!(key.kind, item::ROOT_REF | item::ROOT_BACKREF) && data.len() >= 18 {
                let len = usize::from(u16_at(data, 16));
                let value = (
                    u64_at(data, 0),
                    u64_at(data, 8),
                    data.get(18..18 + len).unwrap_or_default().to_vec(),
                );
                if key.kind == item::ROOT_REF {
                    refs.insert((key.objectid, key.offset), value);
                } else {
                    backs.insert((key.offset, key.objectid), value);
                }
            }
            cursor.advance()?;
        }
        let mut problems = Vec::new();
        for (&(parent, child), value) in &refs {
            if backs.get(&(parent, child)) != Some(value) {
                problems.push(format!("subvolume {child}: its name in {parent} is not named back"));
            }
            if !subvolumes.iter().any(|s| s.0 == child) {
                problems.push(format!("subvolume {child} is named in {parent}, but is not there"));
            }
        }
        for (&(parent, child), (dirid, index, name)) in &backs {
            if !refs.contains_key(&(parent, child)) {
                problems.push(format!("subvolume {child}: its parent {parent} does not name it"));
            }
            // The parent's directory has the entry.
            let Some(&(_, tree_root)) = subvolumes.iter().find(|s| s.0 == parent) else { continue };
            let key = Key::new(*dirid, item::DIR_INDEX, *index);
            let named = crate::tree::find(volume, &tree_root, &key)?
                .and_then(|d| dir_entries(&d))
                .is_some_and(|e| {
                    e.iter().any(|(location, _, n)| {
                        n == name && location.objectid == child && location.kind == item::ROOT_ITEM
                    })
                });
            if !named {
                problems.push(format!(
                    "subvolume {child}: directory {dirid} of {parent} has no entry {} at {index}",
                    String::from_utf8_lossy(name)
                ));
            }
        }
        for &(id, _) in subvolumes {
            if (tree::FIRST_FREE..LAST_FREE).contains(&id) && !backs.keys().any(|k| k.1 == id) {
                problems.push(format!("subvolume {id} has no parent"));
            }
        }
        self.report.problems.extend(problems);
        Ok(())
    }
}
