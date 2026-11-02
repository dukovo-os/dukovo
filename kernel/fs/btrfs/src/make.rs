// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Making a filesystem (Phase 14a, ADR 0304): an empty Btrfs on a
//! device, as `mkfs.btrfs` makes one, for `mkfs-btrfs` and `./ob image`
//! (feature `make`). Everything after, from subvolumes to files, is the
//! writer's ([`crate::cow::Writer`]), on the filesystem mounted.
//!
//! The device gets three chunks, each a block group, one after the other
//! from 1 MiB (the first MiB is left to boot loaders, as on every Btrfs):
//!
//! - the system chunk, two copies of 8 MiB, which holds the chunk tree;
//! - the metadata chunk, two copies (one if `dup` is off) of the size
//!   `mkfs.btrfs` 6.17 gives it: 256 MiB, or 1 GiB on a device over
//!   50 GiB, but half a tenth of the device when that is less, and 32 MiB
//!   at least;
//! - the data chunk, of 8 MiB. The writer adds chunks as files fill it.
//!
//! The trees are those `mkfs.btrfs` makes, each one leaf, with the same
//! items, as its `btrfs inspect-internal dump-tree` shows them: the root
//! tree, with each tree's root item and the directory that names the top
//! subvolume `default`; the chunk tree, with the device and the chunks;
//! the extent tree, with each block group and each tree block; the device
//! tree, with the device's error counts and each stripe; the top
//! subvolume and the data relocation tree, an empty directory each; an
//! empty checksum tree; the UUID tree, naming the top subvolume; and the
//! free-space tree, with each group's free space but the superblock
//! copies', as the writer keeps it ([`crate::space`]). The features are
//! `mkfs.btrfs`'s defaults: skinny metadata, extended references, no
//! holes and the free-space tree.
//!
//! The blocks are written and flushed first, then the superblock and its
//! copies at 64 MiB and 256 GiB, where the device reaches them: until
//! then, the device holds no filesystem.

use radix_block::DeviceRef;
use radix_block::cache::BlockCache;
use radix_vfs::mem::{self, KVec};
use radix_vfs::{Error, Result};

use crate::chunk::{Chunk, Stripe, kind};
use crate::csum::{CSUM_SIZE, CsumType};
use crate::format::{Key, item, name_hash, tree};
use crate::superblock::{self, LABEL_MAX, MAGIC, SYS_CHUNK_ARRAY_MAX, compat_ro, incompat};
use crate::tree::{Block, ITEM_SIZE};

const MIB: u64 = 1 << 20;

/// The smallest device a filesystem is made on, as `mkfs.btrfs` has it:
/// room for the three chunks, and for the writer to add more.
pub const MIN_BYTES: u64 = 109 * MIB;

/// The sector size: the page size, the only one Radix mounts.
pub const SECTORSIZE: u32 = 4096;

/// The tree block size `mkfs.btrfs` makes.
pub const NODESIZE: u32 = 16 << 10;

/// The stripe length of every chunk, as Linux has it.
const STRIPE_LEN: u64 = 64 << 10;

/// The transaction everything is made in.
const GENERATION: u64 = 1;

/// The data relocation tree, which balance moves data through.
const DATA_RELOC: u64 = (-9i64) as u64;

/// The object ID of the chunk tree's device items.
const DEV_ITEMS: u64 = 1;

/// The object ID of the device tree's error counts.
const DEV_STATS: u64 = 0;

/// The item type of the device tree's error counts.
const PERSISTENT_ITEM: u8 = 249;

/// The UUID tree's item type of a subvolume by its UUID.
const UUID_SUBVOL: u8 = 251;

/// A subvolume's top directory.
const TOP: u64 = 256;

/// The sizes of a root item, as Linux has written them since 3.5, of an
/// inode item, and of a device item.
const ROOT_ITEM_SIZE: usize = 439;
const INODE_ITEM_SIZE: usize = 160;
const DEV_ITEM_SIZE: usize = 98;

/// The size of a device extent item.
const DEV_EXTENT_SIZE: usize = 48;

/// The inode flags of a subvolume's root item, Linux's
/// `BTRFS_INODE_ROOT_ITEM_INIT`.
const ROOT_ITEM_INIT: u64 = 1 << 31;

/// An extent item's flag for a tree block.
const TREE_BLOCK: u64 = 2;

/// The file type byte of a directory, in a directory entry.
const FT_DIR: u8 = 2;

/// The mode of a directory, `rwxr-xr-x`.
const DIR_MODE: u32 = 0o40755;

/// The superblock's copies, by where they are on a device, and the
/// stripe's length around each that no block takes ([`crate::space`]).
const MIRRORS: [u64; 3] = [64 << 10, 64 << 20, 256 << 30];
const MIRROR_GUARD: u64 = 64 << 10;

/// What the superblock keeps where.
mod at {
    pub const FSID: usize = 0x20;
    pub const BYTENR: usize = 0x30;
    pub const FLAGS: usize = 0x38;
    pub const MAGIC: usize = 0x40;
    pub const GENERATION: usize = 0x48;
    pub const ROOT: usize = 0x50;
    pub const CHUNK_ROOT: usize = 0x58;
    pub const TOTAL_BYTES: usize = 0x70;
    pub const BYTES_USED: usize = 0x78;
    pub const ROOT_DIR: usize = 0x80;
    pub const NUM_DEVICES: usize = 0x88;
    pub const SECTORSIZE: usize = 0x90;
    pub const NODESIZE: usize = 0x94;
    pub const LEAFSIZE: usize = 0x98;
    pub const STRIPESIZE: usize = 0x9c;
    pub const SYS_ARRAY_SIZE: usize = 0xa0;
    pub const CHUNK_ROOT_GENERATION: usize = 0xa4;
    pub const COMPAT_RO: usize = 0xb4;
    pub const INCOMPAT: usize = 0xbc;
    pub const CSUM_TYPE: usize = 0xc4;
    pub const DEV_ITEM: usize = 0xc9;
    pub const LABEL: usize = 0x12b;
    pub const SYS_CHUNK_ARRAY: usize = 0x32b;
    pub const BACKUPS: usize = 0xb2b;
}

/// What the tree header keeps where, past the checksum.
mod header {
    pub const FSID: usize = 0x20;
    pub const CHUNK_TREE_UUID: usize = 0x40;
}

/// What to make.
#[derive(Clone, Debug)]
pub struct Options<'a> {
    /// The label, at most 255 bytes; empty for none.
    pub label: &'a [u8],
    /// The tree block size: a power of two from 4 KiB to 64 KiB.
    pub nodesize: u32,
    /// The checksum of every block.
    pub csum: CsumType,
    /// Two copies of metadata, as `mkfs.btrfs` makes on one device, or
    /// one.
    pub dup: bool,
    /// The filesystem's UUID.
    pub fsid: [u8; 16],
    /// The device's UUID.
    pub dev_uuid: [u8; 16],
    /// The chunk tree's UUID, which every tree block carries.
    pub chunk_tree_uuid: [u8; 16],
    /// The top subvolume's UUID.
    pub top_uuid: [u8; 16],
    /// The time it is made, in seconds since 1970: the top directories'
    /// times.
    pub time: u64,
}

impl<'a> Options<'a> {
    /// What `mkfs.btrfs` makes by default, labelled `label`, at `time`,
    /// with new random UUIDs ([`radix_vfs::random`]).
    pub fn new(label: &'a [u8], time: u64) -> Options<'a> {
        Options {
            label,
            nodesize: NODESIZE,
            csum: CsumType::Crc32c,
            dup: true,
            fsid: new_uuid(),
            dev_uuid: new_uuid(),
            chunk_tree_uuid: new_uuid(),
            top_uuid: new_uuid(),
            time,
        }
    }
}

/// A random UUID, of version 4 (RFC 9562).
pub fn new_uuid() -> [u8; 16] {
    let mut uuid = [0; 16];
    radix_vfs::random(&mut uuid);
    uuid[6] = uuid[6] & 0x0f | 0x40;
    uuid[8] = uuid[8] & 0x3f | 0x80;
    uuid
}

/// One of the chunks made, and its block group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    /// What it holds and its profile ([`kind`]).
    pub flags: u64,
    /// Its first logical address.
    pub start: u64,
    /// Its length.
    pub length: u64,
    /// Where its copies are on the device.
    pub stripes: [u64; 2],
    /// How many copies it has: 1 or 2.
    pub copies: usize,
}

impl Group {
    fn end(&self) -> u64 {
        self.start + self.length
    }

    /// The logical ranges around the superblock's copies.
    fn mirrors(&self) -> KVec<(u64, u64)> {
        let mut out = mem::vec();
        for mirror in MIRRORS {
            for &stripe in &self.stripes[..self.copies] {
                let (lo, hi) = (mirror.max(stripe), (mirror + MIRROR_GUARD).min(stripe + self.length));
                if lo < hi {
                    let _ = mem::push(&mut out, (self.start + lo - stripe, self.start + hi - stripe));
                }
            }
        }
        out.sort_unstable();
        out
    }
}

/// The filesystem made on a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Its size: the device's, in whole sectors.
    pub total_bytes: u64,
    /// The system chunk, the metadata chunk and the data chunk.
    pub groups: [Group; 3],
}

impl Layout {
    /// The bytes of the device its chunks take.
    pub fn allocated(&self) -> u64 {
        self.groups.iter().map(|g| g.length * g.copies as u64).sum()
    }
}

/// The chunks of a filesystem on a device of `bytes`, with two copies of
/// metadata if `dup`.
///
/// Fails with [`Error::NoSpace`] for a device under [`MIN_BYTES`].
pub fn layout(bytes: u64, dup: bool) -> Result<Layout> {
    let total = bytes / u64::from(SECTORSIZE) * u64::from(SECTORSIZE);
    if total < MIN_BYTES {
        return Err(Error::NoSpace);
    }
    let copies = if dup { 2 } else { 1 };
    let profile = if dup { kind::DUP } else { 0 };
    let system = Group {
        flags: kind::SYSTEM | profile,
        start: MIB,
        length: 8 * MIB,
        stripes: [MIB, 9 * MIB],
        copies,
    };
    let at = MIB + system.length * copies as u64;
    let length = metadata_length(total, copies as u64);
    let metadata = Group {
        flags: kind::METADATA | profile,
        start: system.end(),
        length,
        stripes: [at, at + length],
        copies,
    };
    let at = at + length * copies as u64;
    let data =
        Group { flags: kind::DATA, start: metadata.end(), length: 8 * MIB, stripes: [at, 0], copies: 1 };
    if at + data.length > total {
        return Err(Error::NoSpace);
    }
    Ok(Layout { total_bytes: total, groups: [system, metadata, data] })
}

/// The length of the first metadata chunk on a device of `total` bytes,
/// with `copies` copies, as `mkfs.btrfs` 6.17 makes it: 256 MiB, or
/// 1 GiB over 50 GiB, unless that is more than a tenth of the device, when
/// all its copies take a tenth; never less than 32 MiB.
fn metadata_length(total: u64, copies: u64) -> u64 {
    let want = if total > 50 << 30 { 1 << 30 } else { 256 * MIB };
    let tenth = total / 10;
    let length = if want > tenth { tenth / copies / STRIPE_LEN * STRIPE_LEN } else { want };
    length.max(32 * MIB)
}

/// A leaf to be: its tree, where it goes, and its items.
struct Leaf {
    owner: u64,
    bytenr: u64,
    items: KVec<(Key, KVec<u8>)>,
}

impl Leaf {
    fn new(owner: u64, bytenr: u64) -> Leaf {
        Leaf { owner, bytenr, items: mem::vec() }
    }

    /// Adds item `key`, of `size` bytes that `fill` writes.
    fn add(&mut self, key: Key, size: usize, fill: impl FnOnce(&mut [u8])) -> Result<()> {
        let mut data = crate::zeroed(size)?;
        fill(&mut data);
        mem::push(&mut self.items, (key, data))?;
        Ok(())
    }

    /// The block, its items in order, sealed with its checksum.
    fn block(mut self, o: &Options<'_>) -> Result<Block> {
        let mut bytes = crate::zeroed(o.nodesize as usize)?;
        bytes[header::FSID..header::FSID + 16].copy_from_slice(&o.fsid);
        bytes[header::CHUNK_TREE_UUID..header::CHUNK_TREE_UUID + 16]
            .copy_from_slice(&o.chunk_tree_uuid);
        let mut block = Block::from_bytes(self.bytenr, bytes);
        block.set_header(self.bytenr, GENERATION, self.owner, 0);
        self.items.sort_unstable_by_key(|item| item.0);
        for (slot, (key, data)) in self.items.iter().enumerate() {
            if block.leaf_free() < data.len() + ITEM_SIZE {
                return Err(Error::NoSpace);
            }
            block.insert_item(slot, key, data.len()).copy_from_slice(data);
        }
        block.seal(o.csum);
        Ok(block)
    }
}

fn put_u16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// An inode reference at index 0, naming its inode `name`, as a
/// directory's own `..` and the top subvolume's `default` are.
fn inode_ref(name: &[u8]) -> impl FnOnce(&mut [u8]) + '_ {
    move |d: &mut [u8]| {
        put_u16(d, 8, name.len() as u16);
        d[10..].copy_from_slice(name);
    }
}

fn put_key(bytes: &mut [u8], at: usize, key: &Key) {
    put_u64(bytes, at, key.objectid);
    bytes[at + 8] = key.kind;
    put_u64(bytes, at + 9, key.offset);
}

/// An empty directory's inode item, at `time` if it has one, of `nodesize`
/// bytes of metadata, as `mkfs.btrfs` fills a root item's.
fn directory(d: &mut [u8], nodesize: u32, time: Option<u64>) {
    put_u64(d, 0, GENERATION);
    put_u64(d, 24, u64::from(nodesize));
    put_u32(d, 40, 1);
    put_u32(d, 52, DIR_MODE);
    if let Some(time) = time {
        // Access, change, modification and creation.
        for at in [112, 124, 136, 148] {
            put_u64(d, at, time);
        }
    }
}

/// A root item for a tree whose root block is `bytenr`.
struct RootItem {
    bytenr: u64,
    /// Its top directory, for a subvolume.
    dirid: u64,
    /// Whether its inode item is filled, as `mkfs.btrfs` fills every
    /// tree's but the UUID tree's and the data relocation tree's: a
    /// directory of size 3.
    inode: bool,
    flags: u64,
    /// The UUID and the time made, of a subvolume.
    made: Option<([u8; 16], u64)>,
}

impl RootItem {
    fn fill(&self, d: &mut [u8], nodesize: u32) {
        if self.inode {
            directory(d, nodesize, None);
            put_u64(d, 16, 3);
        }
        put_u64(d, 64, self.flags);
        put_u64(d, 160, GENERATION);
        put_u64(d, 168, self.dirid);
        put_u64(d, 176, self.bytenr);
        put_u64(d, 192, u64::from(nodesize));
        put_u32(d, 216, 1);
        put_u64(d, 239, GENERATION);
        if let Some((uuid, time)) = self.made {
            d[247..263].copy_from_slice(&uuid);
            put_u64(d, 327, time);
            put_u64(d, 339, time);
        }
    }
}

/// Makes the filesystem `o` says on `device`, the whole of it, and
/// returns its layout. The first MiB is cleared, so that nothing reads
/// the device as what it held before.
///
/// Fails with [`Error::ReadOnly`] for a device that refuses writes, with
/// [`Error::NotSupported`] for one whose logical blocks are larger than
/// 4 KiB, with [`Error::InvalidArgument`] for a node size or label `o`
/// should not have, as [`layout`] does, and as writing fails.
pub fn make(device: &DeviceRef, o: &Options<'_>) -> Result<Layout> {
    let info = device.info();
    if info.read_only {
        return Err(Error::ReadOnly);
    }
    if info.logical_block > SECTORSIZE {
        return Err(Error::NotSupported);
    }
    let nodesize_fits =
        o.nodesize.is_power_of_two() && (SECTORSIZE..=STRIPE_LEN as u32).contains(&o.nodesize);
    if !nodesize_fits || o.label.len() >= LABEL_MAX || o.label.contains(&0) {
        return Err(Error::InvalidArgument);
    }
    let layout = layout(info.sectors.saturating_mul(512), o.dup)?;
    let [system, metadata, data] = layout.groups;
    let nodesize = u64::from(o.nodesize);

    // Where each tree's block goes: the chunk tree's in the system chunk,
    // the others' one after another in the metadata chunk, around the
    // superblock's copies.
    let place = |group: &Group, count: usize| -> Result<KVec<u64>> {
        let mirrors = group.mirrors();
        let mut out = mem::vec_with_capacity(count)?;
        let mut at = group.start;
        while out.len() < count {
            if let Some(&(_, hi)) = mirrors.iter().find(|&&(lo, hi)| at < hi && lo < at + nodesize) {
                at = hi.div_ceil(nodesize) * nodesize;
                continue;
            }
            if at + nodesize > group.end() {
                return Err(Error::NoSpace);
            }
            mem::push(&mut out, at)?;
            at += nodesize;
        }
        Ok(out)
    };
    let chunk_root = place(&system, 1)?[0];
    const TREES: [u64; 8] = [
        tree::ROOT,
        tree::EXTENT,
        tree::DEV,
        tree::FS,
        tree::CSUM,
        tree::UUID,
        tree::FREE_SPACE,
        DATA_RELOC,
    ];
    let blocks = place(&metadata, TREES.len())?;
    let at = |id: u64| blocks[TREES.iter().position(|&t| t == id).unwrap_or(0)];
    let bytes_used = (1 + TREES.len() as u64) * nodesize;

    let mut chunks = mem::vec_with_capacity::<Chunk>(3)?;
    for g in [system, metadata, data] {
        let mut stripes = mem::vec_with_capacity(g.copies)?;
        for &offset in &g.stripes[..g.copies] {
            mem::push(&mut stripes, Stripe { devid: 1, offset })?;
        }
        let chunk = Chunk {
            start: g.start,
            length: g.length,
            stripe_len: STRIPE_LEN,
            kind: g.flags,
            sub_stripes: 1,
            stripes,
        };
        mem::push(&mut chunks, chunk)?;
    }
    let dev_item = |d: &mut [u8]| {
        put_u64(d, 0, 1);
        put_u64(d, 8, layout.total_bytes);
        put_u64(d, 16, layout.allocated());
        for at in [24, 28, 32] {
            put_u32(d, at, SECTORSIZE);
        }
        d[66..82].copy_from_slice(&o.dev_uuid);
        d[82..98].copy_from_slice(&o.fsid);
    };

    let mut leaves = mem::vec_with_capacity::<Leaf>(1 + TREES.len())?;

    // The chunk tree: the device, and the chunks.
    let mut leaf = Leaf::new(tree::CHUNK, chunk_root);
    leaf.add(Key::new(DEV_ITEMS, item::DEV_ITEM, 1), DEV_ITEM_SIZE, dev_item)?;
    for chunk in &chunks {
        let key = Key::new(tree::FIRST_CHUNK, item::CHUNK_ITEM, chunk.start);
        let size = Chunk::item_size(chunk.stripes.len());
        leaf.add(key, size, |d| chunk.write(d, SECTORSIZE, |_| o.dev_uuid))?;
    }
    mem::push(&mut leaves, leaf)?;

    // The root tree: each tree's root item, and the directory naming the
    // top subvolume.
    let mut leaf = Leaf::new(tree::ROOT, at(tree::ROOT));
    for id in TREES.into_iter().filter(|&id| id != tree::ROOT) {
        let root = RootItem {
            bytenr: at(id),
            dirid: if matches!(id, tree::FS | DATA_RELOC) { TOP } else { 0 },
            inode: !matches!(id, tree::UUID | DATA_RELOC),
            flags: if matches!(id, tree::FS | DATA_RELOC) { ROOT_ITEM_INIT } else { 0 },
            made: (id == tree::FS).then_some((o.top_uuid, o.time)),
        };
        leaf.add(Key::new(id, item::ROOT_ITEM, 0), ROOT_ITEM_SIZE, |d| root.fill(d, o.nodesize))?;
    }
    const DEFAULT: &[u8] = b"default";
    leaf.add(Key::new(tree::FS, item::INODE_REF, tree::ROOT_TREE_DIR), 10 + DEFAULT.len(), inode_ref(DEFAULT))?;
    leaf.add(Key::new(tree::ROOT_TREE_DIR, item::INODE_ITEM, 0), INODE_ITEM_SIZE, |d| {
        directory(d, o.nodesize, Some(o.time));
    })?;
    leaf.add(Key::new(tree::ROOT_TREE_DIR, item::INODE_REF, tree::ROOT_TREE_DIR), 12, inode_ref(b".."))?;
    let entry = Key::new(tree::ROOT_TREE_DIR, item::DIR_ITEM, name_hash(DEFAULT));
    leaf.add(entry, 30 + DEFAULT.len(), |d| {
        put_key(d, 0, &Key::new(tree::FS, item::ROOT_ITEM, u64::MAX));
        put_u64(d, 17, GENERATION);
        put_u16(d, 27, DEFAULT.len() as u16);
        d[29] = FT_DIR;
        d[30..].copy_from_slice(DEFAULT);
    })?;
    mem::push(&mut leaves, leaf)?;

    // The extent tree: each block group, and each tree block.
    let mut leaf = Leaf::new(tree::EXTENT, at(tree::EXTENT));
    for (group, used) in [(system, 1), (metadata, TREES.len() as u64), (data, 0)] {
        leaf.add(Key::new(group.start, item::BLOCK_GROUP_ITEM, group.length), 24, |d| {
            put_u64(d, 0, used * nodesize);
            put_u64(d, 8, tree::FIRST_CHUNK);
            put_u64(d, 16, group.flags);
        })?;
    }
    let owners = core::iter::once((chunk_root, tree::CHUNK)).chain(TREES.iter().map(|&id| (at(id), id)));
    for (bytenr, owner) in owners {
        leaf.add(Key::new(bytenr, item::METADATA_ITEM, 0), 33, |d| {
            put_u64(d, 0, 1);
            put_u64(d, 8, GENERATION);
            put_u64(d, 16, TREE_BLOCK);
            d[24] = item::TREE_BLOCK_REF;
            put_u64(d, 25, owner);
        })?;
    }
    mem::push(&mut leaves, leaf)?;

    // The device tree: its error counts, and each stripe.
    let mut leaf = Leaf::new(tree::DEV, at(tree::DEV));
    leaf.add(Key::new(DEV_STATS, PERSISTENT_ITEM, 1), 40, |_| {})?;
    for group in [system, metadata, data] {
        for &offset in &group.stripes[..group.copies] {
            leaf.add(Key::new(1, item::DEV_EXTENT, offset), DEV_EXTENT_SIZE, |d| {
                put_u64(d, 0, tree::CHUNK);
                put_u64(d, 8, tree::FIRST_CHUNK);
                put_u64(d, 16, group.start);
                put_u64(d, 24, group.length);
                d[32..48].copy_from_slice(&o.chunk_tree_uuid);
            })?;
        }
    }
    mem::push(&mut leaves, leaf)?;

    // The top subvolume and the data relocation tree: an empty directory
    // each.
    for id in [tree::FS, DATA_RELOC] {
        let mut leaf = Leaf::new(id, at(id));
        leaf.add(Key::new(TOP, item::INODE_ITEM, 0), INODE_ITEM_SIZE, |d| {
            directory(d, o.nodesize, Some(o.time));
        })?;
        leaf.add(Key::new(TOP, item::INODE_REF, TOP), 12, inode_ref(b".."))?;
        mem::push(&mut leaves, leaf)?;
    }

    // The checksum tree, empty, and the UUID tree, naming the top
    // subvolume.
    mem::push(&mut leaves, Leaf::new(tree::CSUM, at(tree::CSUM)))?;
    let mut leaf = Leaf::new(tree::UUID, at(tree::UUID));
    let (high, low) = (crate::format::u64_at(&o.top_uuid, 0), crate::format::u64_at(&o.top_uuid, 8));
    leaf.add(Key::new(high, UUID_SUBVOL, low), 8, |d| put_u64(d, 0, tree::FS))?;
    mem::push(&mut leaves, leaf)?;

    // The free-space tree: each group's free ranges, as extents.
    let mut leaf = Leaf::new(tree::FREE_SPACE, at(tree::FREE_SPACE));
    for group in [system, metadata, data] {
        let mut taken = group.mirrors();
        let tree_blocks =
            core::iter::once(chunk_root).chain(blocks.iter().copied()).filter(|&b| b >= group.start && b < group.end());
        for bytenr in tree_blocks {
            mem::push(&mut taken, (bytenr, bytenr + nodesize))?;
        }
        taken.sort_unstable();
        let mut free = mem::vec::<(u64, u64)>();
        let mut at = group.start;
        for &(lo, hi) in &taken {
            if lo > at {
                mem::push(&mut free, (at, lo))?;
            }
            at = at.max(hi);
        }
        if at < group.end() {
            mem::push(&mut free, (at, group.end()))?;
        }
        leaf.add(Key::new(group.start, item::FREE_SPACE_INFO, group.length), 8, |d| {
            put_u32(d, 0, free.len() as u32);
        })?;
        for &(lo, hi) in &free {
            leaf.add(Key::new(lo, item::FREE_SPACE_EXTENT, hi - lo), 0, |_| {})?;
        }
    }
    mem::push(&mut leaves, leaf)?;

    // The blocks, to each copy of their chunk, and the first MiB cleared.
    let cache = BlockCache::new(device.clone());
    cache.fill(0, MIB as usize, 0)?;
    for leaf in leaves {
        let group = if leaf.owner == tree::CHUNK { system } else { metadata };
        let block = leaf.block(o)?;
        for &stripe in &group.stripes[..group.copies] {
            cache.write(stripe + block.bytenr() - group.start, block.bytes())?;
        }
    }
    cache.sync()?;

    // The superblock, last.
    let mut sb = crate::zeroed(superblock::SIZE)?;
    sb[at::FSID..at::FSID + 16].copy_from_slice(&o.fsid);
    put_u64(&mut sb, at::FLAGS, crate::edit::WRITTEN);
    put_u64(&mut sb, at::MAGIC, MAGIC);
    put_u64(&mut sb, at::GENERATION, GENERATION);
    put_u64(&mut sb, at::ROOT, at(tree::ROOT));
    put_u64(&mut sb, at::CHUNK_ROOT, chunk_root);
    put_u64(&mut sb, at::TOTAL_BYTES, layout.total_bytes);
    put_u64(&mut sb, at::BYTES_USED, bytes_used);
    put_u64(&mut sb, at::ROOT_DIR, tree::ROOT_TREE_DIR);
    put_u64(&mut sb, at::NUM_DEVICES, 1);
    put_u32(&mut sb, at::SECTORSIZE, SECTORSIZE);
    put_u32(&mut sb, at::NODESIZE, o.nodesize);
    put_u32(&mut sb, at::LEAFSIZE, o.nodesize);
    put_u32(&mut sb, at::STRIPESIZE, SECTORSIZE);
    put_u64(&mut sb, at::CHUNK_ROOT_GENERATION, GENERATION);
    put_u64(&mut sb, at::COMPAT_RO, compat_ro::FREE_SPACE_TREE | compat_ro::FREE_SPACE_TREE_VALID);
    let mut features =
        incompat::MIXED_BACKREF | incompat::EXTENDED_IREF | incompat::SKINNY_METADATA | incompat::NO_HOLES;
    if o.nodesize > SECTORSIZE {
        features |= incompat::BIG_METADATA;
    }
    put_u64(&mut sb, at::INCOMPAT, features);
    put_u16(&mut sb, at::CSUM_TYPE, o.csum.to_disk());
    dev_item(&mut sb[at::DEV_ITEM..at::DEV_ITEM + DEV_ITEM_SIZE]);
    sb[at::LABEL..at::LABEL + o.label.len()].copy_from_slice(o.label);
    // The system chunk, where reading starts.
    let array = &mut sb[at::SYS_CHUNK_ARRAY..at::SYS_CHUNK_ARRAY + SYS_CHUNK_ARRAY_MAX];
    put_key(array, 0, &Key::new(tree::FIRST_CHUNK, item::CHUNK_ITEM, system.start));
    let size = Chunk::item_size(system.copies);
    chunks[0].write(&mut array[Key::SIZE..Key::SIZE + size], SECTORSIZE, |_| o.dev_uuid);
    put_u32(&mut sb, at::SYS_ARRAY_SIZE, (Key::SIZE + size) as u32);
    // The first backup root, as the writer fills them (crate::commit).
    let b = at::BACKUPS;
    let roots = [
        (0, 152, at(tree::ROOT)),
        (16, 153, chunk_root),
        (32, 154, at(tree::EXTENT)),
        (48, 155, at(tree::FS)),
        (64, 156, at(tree::DEV)),
        (80, 157, at(tree::CSUM)),
    ];
    for (offset, _level, bytenr) in roots {
        put_u64(&mut sb, b + offset, bytenr);
        put_u64(&mut sb, b + offset + 8, GENERATION);
    }
    put_u64(&mut sb, b + 96, layout.total_bytes);
    put_u64(&mut sb, b + 104, bytes_used);
    put_u64(&mut sb, b + 112, 1);
    for mirror in MIRRORS {
        if mirror + superblock::SIZE as u64 > layout.total_bytes {
            continue;
        }
        put_u64(&mut sb, at::BYTENR, mirror);
        let sum = o.csum.sum(&sb[CSUM_SIZE..]);
        sb[..CSUM_SIZE].copy_from_slice(&sum);
        cache.write(mirror, &sb)?;
    }
    cache.sync()?;
    Ok(layout)
}
