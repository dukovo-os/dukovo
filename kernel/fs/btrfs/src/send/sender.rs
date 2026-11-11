// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Sending a read-only snapshot (ADR 0462): the commands that make it on
//! the receiver, whole or from a parent snapshot the receiver already
//! holds, from the items the two snapshots' trees differ by.
//!
//! The receiver applies each command to the subvolume as the commands
//! before it left it, by path. So the sender keeps where each folder it
//! touched stands on the receiver, and goes in four steps:
//!
//! 1. Out of the way. Every folder that moves or goes, and every file
//!    that moves but keeps none of its names, is renamed to a name of its
//!    own at the top, `o<inode>-<generation>-<n>` (as Linux names
//!    orphans); every other name that goes is unlinked. Then the folders
//!    that go, empty now, are removed.
//! 2. Folders into place, the shallowest first: new ones made, moved ones
//!    renamed from the top.
//! 3. Files into place: those that moved renamed from the top, and the
//!    new names of the others linked.
//! 4. Contents, file by file in the order of their inode numbers, from a
//!    second walk of the differences: new files made, extended attributes
//!    set and removed, the bytes that changed written, sizes, owners and
//!    permissions set, and the times of every file. The folders' times
//!    come last, since what they hold changes until the end.
//!
//! A file is its inode number and generation: a number used again for
//! another file (the old one removed, a new one made) is two files, and
//! the old one goes by its number with [`GHOST`] set.
//!
//! The sender keeps the folders and files whose names change, and a full
//! send every folder; the files' contents stream through.

use std::collections::{BTreeMap, BTreeSet};
use std::format;
use std::io::{self, Write};
use std::vec;
use std::vec::Vec;

use crate::format::{Key, item, name_hash, tree};

use super::items::{self, Extent, Inode, corrupt, kind};
use super::stream::{Command, MAX_WRITE, Time, Writer};

/// The inode number of a subvolume's top folder.
pub const TOP: u64 = tree::FIRST_FREE;

/// Set in the number of the old file of an inode number used again.
const GHOST: u64 = 1 << 63;

/// The deepest a folder may be: past it, a tree's names go round.
const MAX_DEPTH: usize = 4096;

/// One item two trees differ by, or one tree's item: its key, and its data
/// in the old tree and the new, `None` where that tree has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Its key.
    pub key: Key,
    /// Its data in the old tree.
    pub old: Option<Vec<u8>>,
    /// Its data in the new tree.
    pub new: Option<Vec<u8>>,
}

/// Which trees a walk reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trees {
    /// The items the parent's tree and the snapshot's differ by, or every
    /// item of the snapshot's, as new, with no parent.
    Both,
    /// The parent's items, as old.
    Old,
    /// The snapshot's items, as new.
    New,
}

/// What a send reads: the trees of the snapshot and its parent, and the
/// snapshot's files.
pub trait Source {
    /// Gives `out` the items of `trees` from key `from` to `to`, both
    /// included, in key order: some of them, at least one if there are
    /// any. Returns true once it gave the last.
    fn compare(
        &mut self,
        trees: Trees,
        from: &Key,
        to: &Key,
        out: &mut Vec<Item>,
    ) -> io::Result<bool>;

    /// Reads the snapshot's file `path` from `offset`, as much as `out`
    /// holds; returns how much it read, less only at the file's end.
    fn read(&mut self, path: &[u8], offset: u64, out: &mut [u8]) -> io::Result<usize>;

    /// Where the snapshot's symbolic link `path` points.
    fn read_link(&mut self, path: &[u8]) -> io::Result<Vec<u8>>;
}

/// What a send says of a subvolume.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Subvolume {
    /// Its UUID.
    pub uuid: [u8; 16],
    /// The UUID of the subvolume it was received as, for one received.
    pub received_uuid: [u8; 16],
    /// The transaction that last changed a file in it.
    pub changed_transaction: u64,
}

impl Subvolume {
    /// The UUID a stream gives it: the one it was received as, if it was,
    /// so that a copy of a copy has its parent where the first copy went.
    pub fn sent_uuid(&self) -> [u8; 16] {
        if self.received_uuid == [0; 16] { self.uuid } else { self.received_uuid }
    }
}

/// How much a send wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sent {
    /// The commands written.
    pub commands: u64,
    /// The files' bytes written.
    pub data: u64,
}

/// Sends `snapshot` from `source` to `out`, whole or, with a `parent`,
/// as the changes from it, as the subvolume `name` on the receiver.
pub fn send<S: Source, W: Write>(
    source: &mut S,
    name: &[u8],
    snapshot: &Subvolume,
    parent: Option<&Subvolume>,
    out: &mut Writer<W>,
) -> io::Result<Sent> {
    let mut sender = Sender {
        source,
        out,
        sent: Sent::default(),
        changes: BTreeMap::new(),
        replaced: BTreeSet::new(),
        entities: BTreeMap::new(),
        place: BTreeMap::new(),
        orphans: BTreeSet::new(),
        old_at: BTreeMap::new(),
        new_at: BTreeMap::new(),
        touched: BTreeSet::new(),
        dir_times: BTreeMap::new(),
        buffer: vec![0; MAX_WRITE],
    };
    let path = name.to_vec();
    let (uuid, ctransid) = (snapshot.sent_uuid(), snapshot.changed_transaction);
    match parent {
        None => sender.put(&Command::Subvol { path, uuid, ctransid })?,
        Some(parent) => sender.put(&Command::Snapshot {
            path,
            uuid,
            ctransid,
            clone_uuid: parent.sent_uuid(),
            clone_ctransid: parent.changed_transaction,
        })?,
    }
    sender.scan()?;
    sender.out_of_the_way()?;
    sender.folders_into_place()?;
    sender.files_into_place()?;
    sender.contents()?;
    sender.put(&Command::End)?;
    Ok(sender.sent)
}

/// The key after `key`, if there is one.
fn after(key: &Key) -> Option<Key> {
    if key.offset < u64::MAX {
        Some(Key::new(key.objectid, key.kind, key.offset + 1))
    } else if key.kind < u8::MAX {
        Some(Key::new(key.objectid, key.kind + 1, 0))
    } else {
        key.objectid.checked_add(1).map(|objectid| Key::new(objectid, 0, 0))
    }
}

/// The last key of a subvolume's files.
fn last_key() -> Key {
    Key::new(tree::LAST_FREE - 1, u8::MAX, u64::MAX)
}

/// The items of a range of keys, read some at a time.
struct Walk {
    trees: Trees,
    next: Option<Key>,
    to: Key,
    batch: Vec<Item>,
    at: usize,
    done: bool,
}

impl Walk {
    fn new(trees: Trees, from: Key, to: Key) -> Walk {
        Walk { trees, next: Some(from), to, batch: Vec::new(), at: 0, done: false }
    }

    /// The next item, if there is one.
    fn next(&mut self, source: &mut impl Source) -> io::Result<Option<Item>> {
        while self.at >= self.batch.len() && !self.done {
            let Some(from) = self.next else {
                self.done = true;
                break;
            };
            self.batch.clear();
            self.at = 0;
            self.done = source.compare(self.trees, &from, &self.to, &mut self.batch)?;
            match self.batch.last() {
                Some(last) if last.key < from || last.key > self.to => {
                    return Err(corrupt("compared"));
                }
                Some(last) => self.next = after(&last.key),
                None if !self.done => {
                    return Err(io::Error::other("the trees gave nothing to compare"));
                }
                None => {}
            }
        }
        let Some(item) = self.batch.get_mut(self.at) else { return Ok(None) };
        self.at += 1;
        Ok(Some(Item { key: item.key, old: item.old.take(), new: item.new.take() }))
    }
}

/// A name: the folder it is in, and the name.
type Name = (u64, Vec<u8>);

/// A file whose names change: one that moves, is made or goes, or a
/// folder that is made (every folder, in a full send).
#[derive(Clone, Debug, Default)]
struct Entity {
    old: Option<Inode>,
    new: Option<Inode>,
    /// Whether the names are all known: a file made or gone has all its
    /// names in the differences.
    names_known: bool,
    old_names: Vec<Name>,
    new_names: Vec<Name>,
}

impl Entity {
    fn is_dir(&self) -> bool {
        self.new.or(self.old).is_some_and(|inode| inode.is_dir())
    }

    /// Whether it is in both trees, one file.
    fn same(&self) -> bool {
        matches!((self.old, self.new), (Some(old), Some(new)) if old.generation == new.generation)
    }

    fn removed(&self) -> Vec<Name> {
        self.old_names.iter().filter(|name| !self.new_names.contains(name)).cloned().collect()
    }

    fn added(&self) -> Vec<Name> {
        self.new_names.iter().filter(|name| !self.old_names.contains(name)).cloned().collect()
    }

    fn kept(&self) -> Vec<Name> {
        self.new_names.iter().filter(|name| self.old_names.contains(name)).cloned().collect()
    }
}

/// Where the receiver holds a folder or file the send has moved.
enum Place {
    /// At the top, under a name of its own.
    Orphan(Vec<u8>),
    /// In a folder, under a name.
    At(u64, Vec<u8>),
}

/// A file of step 4, as the differences give it.
#[derive(Default)]
struct File {
    ino: u64,
    /// Gone: nothing to do.
    gone: bool,
    /// Made by this send: new, or the new file of a number used again.
    made: bool,
    old: Option<Inode>,
    new: Option<Inode>,
    /// Whether its inode item changed.
    inode_changed: bool,
    /// Whether its contents or extended attributes changed.
    contents_changed: bool,
    /// The names of a new file, from the differences.
    names: Vec<Name>,
    /// Whether the steps that need its names are taken.
    begun: bool,
    /// Its path on the receiver, which is its path in the snapshot.
    path: Vec<u8>,
    /// Its size on the receiver.
    size: u64,
    /// The bytes changed and not written yet.
    pending: Option<(u64, u64)>,
}

struct Sender<'s, S: Source, W: Write> {
    source: &'s mut S,
    out: &'s mut Writer<W>,
    sent: Sent,
    /// The files whose names change, by inode number, as step 0 finds them.
    changes: BTreeMap<u64, Entity>,
    /// The inode numbers used again.
    replaced: BTreeSet<u64>,
    /// The files whose names change, the old file of a number used again
    /// with [`GHOST`] set.
    entities: BTreeMap<u64, Entity>,
    /// Where the receiver holds what the send moved.
    place: BTreeMap<u64, Place>,
    orphans: BTreeSet<Vec<u8>>,
    /// Where the parent holds folders, as looked up.
    old_at: BTreeMap<u64, Name>,
    /// Where the snapshot holds folders, as looked up.
    new_at: BTreeMap<u64, Name>,
    /// The folders whose entries changed.
    touched: BTreeSet<u64>,
    /// The times of the folders whose inode items changed.
    dir_times: BTreeMap<u64, (Time, Time, Time)>,
    buffer: Vec<u8>,
}

impl<S: Source, W: Write> Sender<'_, S, W> {
    fn put(&mut self, command: &Command) -> io::Result<()> {
        self.out.put(command)?;
        self.sent.commands += 1;
        Ok(())
    }

    /// The items of `trees` from `from` to `to`.
    fn all(&mut self, trees: Trees, from: Key, to: Key) -> io::Result<Vec<Item>> {
        let mut walk = Walk::new(trees, from, to);
        let mut items = Vec::new();
        while let Some(item) = walk.next(self.source)? {
            items.push(item);
        }
        Ok(items)
    }

    /// The inode `ino` of one tree, if it has it.
    fn inode(&mut self, trees: Trees, ino: u64) -> io::Result<Option<Inode>> {
        let key = Key::new(ino, item::INODE_ITEM, 0);
        let items = self.all(trees, key, key)?;
        match items.first().and_then(|item| item.new.as_ref().or(item.old.as_ref())) {
            Some(data) => Inode::parse(data).map(Some),
            None => Ok(None),
        }
    }

    /// The names of inode `ino` in one tree.
    fn names(&mut self, trees: Trees, ino: u64) -> io::Result<Vec<Name>> {
        let from = Key::new(ino, item::INODE_REF, 0);
        let to = Key::new(ino, item::INODE_EXTREF, u64::MAX);
        let mut names = Vec::new();
        for item in self.all(trees, from, to)? {
            if let Some(data) = item.new.as_ref().or(item.old.as_ref()) {
                items::names(item.key.kind, item.key.offset, data, &mut names)?;
            }
        }
        // The top folder's name for itself.
        if ino == TOP {
            names.clear();
        }
        Ok(names)
    }

    /// Whether folder `dir` of one tree has an entry `name`.
    fn has(&mut self, trees: Trees, dir: u64, name: &[u8]) -> io::Result<bool> {
        let key = Key::new(dir, item::DIR_ITEM, name_hash(name));
        for item in self.all(trees, key, key)? {
            if let Some(data) = item.new.as_ref().or(item.old.as_ref())
                && items::dir_entries(data)?.iter().any(|entry| entry.2 == name)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The files folder `dir` of one tree holds, by inode number.
    fn children(&mut self, trees: Trees, dir: u64) -> io::Result<Vec<u64>> {
        let from = Key::new(dir, item::DIR_INDEX, 0);
        let to = Key::new(dir, item::DIR_INDEX, u64::MAX);
        let mut children = Vec::new();
        for item in self.all(trees, from, to)? {
            if let Some(data) = item.new.as_ref().or(item.old.as_ref()) {
                for (ino, kind, _) in items::dir_entries(data)? {
                    // A subvolume's entry: not sent, as Linux does not.
                    if kind == item::INODE_ITEM {
                        children.push(ino);
                    }
                }
            }
        }
        Ok(children)
    }

    /// Step 0: the files whose names change, from a first walk of the
    /// differences, and their names.
    fn scan(&mut self) -> io::Result<()> {
        let mut walk = Walk::new(Trees::Both, Key::new(TOP, 0, 0), last_key());
        let mut group: Option<(u64, Entity, bool, bool)> = None;
        while let Some(item) = walk.next(self.source)? {
            let ino = item.key.objectid;
            if group.as_ref().is_some_and(|g| g.0 != ino)
                && let Some((ino, entity, inode_seen, refs_changed)) = group.take()
            {
                self.record(ino, entity, inode_seen, refs_changed)?;
            }
            if item.key.kind > item::INODE_EXTREF {
                continue;
            }
            let (_, entity, inode_seen, refs_changed) =
                group.get_or_insert_with(|| (ino, Entity::default(), false, false));
            if item.key.kind == item::INODE_ITEM {
                entity.old = item.old.as_deref().map(Inode::parse).transpose()?;
                entity.new = item.new.as_deref().map(Inode::parse).transpose()?;
                *inode_seen = true;
            } else if matches!(item.key.kind, item::INODE_REF | item::INODE_EXTREF) {
                *refs_changed = true;
                if let Some(data) = &item.old {
                    items::names(item.key.kind, item.key.offset, data, &mut entity.old_names)?;
                }
                if let Some(data) = &item.new {
                    items::names(item.key.kind, item.key.offset, data, &mut entity.new_names)?;
                }
            }
        }
        if let Some((ino, entity, inode_seen, refs_changed)) = group.take() {
            self.record(ino, entity, inode_seen, refs_changed)?;
        }
        // A folder's number used again: what the old one held and the new
        // one holds are under another folder, whatever their items say.
        let replaced: Vec<u64> = self.replaced.iter().copied().collect();
        for ino in replaced {
            let Some(entity) = self.changes.get(&ino) else { continue };
            let (old_dir, new_dir) = (
                entity.old.is_some_and(|inode| inode.is_dir()),
                entity.new.is_some_and(|inode| inode.is_dir()),
            );
            let mut children = Vec::new();
            if old_dir {
                children.extend(self.children(Trees::Old, ino)?);
            }
            if new_dir {
                children.extend(self.children(Trees::New, ino)?);
            }
            for child in children {
                if self.changes.contains_key(&child) {
                    continue;
                }
                let old = self.inode(Trees::Old, child)?;
                let new = self.inode(Trees::New, child)?;
                if old.is_none() && new.is_some_and(|inode| !inode.is_dir()) {
                    continue;
                }
                let entity = Entity { old, new, ..Entity::default() };
                self.changes.insert(child, entity);
            }
        }
        // The names of the files that keep their number.
        let inos: Vec<u64> = self.changes.keys().copied().collect();
        for ino in inos {
            let Some(entity) = self.changes.get(&ino) else { continue };
            if entity.names_known {
                continue;
            }
            let (old, new) = (entity.old.is_some(), entity.new.is_some());
            let old_names = if old { self.names(Trees::Old, ino)? } else { Vec::new() };
            let new_names = if new { self.names(Trees::New, ino)? } else { Vec::new() };
            if let Some(entity) = self.changes.get_mut(&ino) {
                entity.old_names = old_names;
                entity.new_names = new_names;
            }
        }
        // The old names in the old folders of numbers used again, and each
        // number used again as two files.
        let changes = std::mem::take(&mut self.changes);
        for (ino, mut entity) in changes {
            for name in &mut entity.old_names {
                if self.replaced.contains(&name.0) {
                    name.0 |= GHOST;
                }
            }
            if self.replaced.contains(&ino) {
                let ghost = Entity {
                    old: entity.old,
                    new: None,
                    names_known: true,
                    old_names: std::mem::take(&mut entity.old_names),
                    new_names: Vec::new(),
                };
                self.entities.insert(ino | GHOST, ghost);
                entity.old = None;
            }
            self.entities.insert(ino, entity);
        }
        Ok(())
    }

    /// Keeps inode `ino` as a file whose names change, if it is one.
    fn record(
        &mut self,
        ino: u64,
        mut entity: Entity,
        inode_seen: bool,
        refs_changed: bool,
    ) -> io::Result<()> {
        if ino >= GHOST {
            return Err(io::Error::other(format!(
                "inode number {ino} is past what a send can name"
            )));
        }
        if !inode_seen {
            // Its inode item is the same in both.
            let inode = match self.inode(Trees::New, ino)? {
                Some(inode) => Some(inode),
                None => self.inode(Trees::Old, ino)?,
            };
            if inode.is_none() {
                return Err(corrupt("inode reference without an inode"));
            }
            (entity.old, entity.new) = (inode, inode);
        }
        // The top folder is never made, moved or removed.
        if ino == TOP {
            return Ok(());
        }
        let keep = match (entity.old, entity.new) {
            (Some(old), Some(new)) if old.generation != new.generation => {
                self.replaced.insert(ino);
                entity.names_known = false;
                true
            }
            (Some(_), Some(_)) => {
                entity.names_known = false;
                refs_changed
            }
            // New files are made in step 4, from the second walk.
            (None, Some(new)) => {
                entity.names_known = true;
                new.is_dir()
            }
            (Some(_), None) => {
                entity.names_known = true;
                true
            }
            (None, None) => false,
        };
        if keep {
            self.changes.insert(ino, entity);
        }
        Ok(())
    }

    /// Where the parent holds file `id`, by the first of its names.
    fn old_location(&mut self, id: u64) -> io::Result<Name> {
        if let Some(name) = self.entities.get(&id).and_then(|e| e.old_names.first()) {
            return Ok(name.clone());
        }
        if let Some(name) = self.old_at.get(&id) {
            return Ok(name.clone());
        }
        let ino = id & !GHOST;
        let mut name =
            self.names(Trees::Old, ino)?.into_iter().next().ok_or_else(|| corrupt("inode"))?;
        if self.replaced.contains(&name.0) {
            name.0 |= GHOST;
        }
        self.old_at.insert(id, name.clone());
        Ok(name)
    }

    /// Where the snapshot holds file `id`, by the first of its names;
    /// `None` for a file with none.
    fn new_location(&mut self, id: u64) -> io::Result<Option<Name>> {
        if let Some(entity) = self.entities.get(&id) {
            return Ok(entity.new_names.first().cloned());
        }
        if let Some(name) = self.new_at.get(&id) {
            return Ok(Some(name.clone()));
        }
        let Some(name) = self.names(Trees::New, id)?.into_iter().next() else { return Ok(None) };
        self.new_at.insert(id, name.clone());
        Ok(Some(name))
    }

    /// The path of folder or file `id` on the receiver, as the commands
    /// so far left it.
    fn path(&mut self, id: u64) -> io::Result<Vec<u8>> {
        let mut parts: Vec<Vec<u8>> = Vec::new();
        let mut at = id;
        for _ in 0..MAX_DEPTH {
            if at == TOP {
                return Ok(join(parts));
            }
            match self.place.get(&at) {
                Some(Place::Orphan(name)) => {
                    parts.push(name.clone());
                    return Ok(join(parts));
                }
                Some(Place::At(parent, name)) => {
                    parts.push(name.clone());
                    at = *parent;
                }
                None => {
                    let (parent, name) = self.old_location(at)?;
                    parts.push(name);
                    at = parent;
                }
            }
        }
        Err(corrupt("folder (one inside itself)"))
    }

    /// The path of `name` in folder `dir` on the receiver.
    fn path_in(&mut self, (dir, name): &Name) -> io::Result<Vec<u8>> {
        let mut path = self.path(*dir)?;
        if !path.is_empty() {
            path.push(b'/');
        }
        path.extend_from_slice(name);
        Ok(path)
    }

    /// Notes that folder `dir`'s entries changed, if it is in the snapshot.
    fn touch(&mut self, dir: u64) {
        if dir & GHOST == 0 && self.entities.get(&dir).is_none_or(|e| e.new.is_some()) {
            self.touched.insert(dir);
        }
    }

    /// A name at the top for `id` that neither tree has there.
    fn orphan_name(&mut self, id: u64) -> io::Result<Vec<u8>> {
        // The top folder holds it for a while, which changes its times.
        self.touched.insert(TOP);
        let generation =
            self.entities.get(&id).and_then(|e| e.old).map_or(0, |inode| inode.generation);
        for n in 0u32.. {
            let name = format!("o{}-{generation}-{n}", id & !GHOST).into_bytes();
            if !self.orphans.contains(&name)
                && !self.has(Trees::Old, TOP, &name)?
                && !self.has(Trees::New, TOP, &name)?
            {
                self.orphans.insert(name.clone());
                return Ok(name);
            }
        }
        Err(io::Error::other("no name left for an orphan"))
    }

    /// Step 1: everything that moves or goes, out of the way.
    fn out_of_the_way(&mut self) -> io::Result<()> {
        let ids: Vec<u64> = self.entities.keys().copied().collect();
        let mut gone_dirs = Vec::new();
        for id in ids {
            let Some(entity) = self.entities.get(&id) else { continue };
            let (dir, same, removed) = (entity.is_dir(), entity.same(), entity.removed());
            let (added, kept) = (entity.added(), entity.kept());
            if removed.is_empty() {
                continue;
            }
            for name in &removed {
                self.touch(name.0);
            }
            if dir || (same && kept.is_empty() && !added.is_empty()) {
                // Moved or gone: to the top, under its own name.
                let from = self.path_in(&removed[0])?;
                let orphan = self.orphan_name(id)?;
                self.put(&Command::Rename { path: from, to: orphan.clone() })?;
                self.place.insert(id, Place::Orphan(orphan));
                if dir && !same {
                    gone_dirs.push(id);
                }
                for name in &removed[1..] {
                    let path = self.path_in(name)?;
                    self.put(&Command::Unlink { path })?;
                }
            } else {
                for name in &removed {
                    let path = self.path_in(name)?;
                    self.put(&Command::Unlink { path })?;
                }
            }
        }
        // Empty now: what they held is gone or moved.
        for id in gone_dirs {
            let path = self.path(id)?;
            self.put(&Command::Rmdir { path })?;
            self.place.remove(&id);
        }
        Ok(())
    }

    /// How deep the snapshot holds folder `id`.
    fn depth(&mut self, id: u64) -> io::Result<usize> {
        let mut at = id;
        for depth in 0..MAX_DEPTH {
            if at == TOP {
                return Ok(depth);
            }
            at = match self.new_location(at)? {
                Some((parent, _)) => parent,
                None => return Err(corrupt("folder (without a name)")),
            };
        }
        Err(corrupt("folder (one inside itself)"))
    }

    /// Step 2: the folders into place, the shallowest first.
    fn folders_into_place(&mut self) -> io::Result<()> {
        let mut folders = Vec::new();
        let ids: Vec<u64> = self.entities.keys().copied().collect();
        for id in ids {
            let Some(entity) = self.entities.get(&id) else { continue };
            if !entity.is_dir() || entity.new.is_none() || entity.added().is_empty() {
                continue;
            }
            folders.push((self.depth(id)?, id));
        }
        folders.sort_unstable();
        for (_, id) in folders {
            let Some(entity) = self.entities.get(&id) else { continue };
            let made = !entity.same();
            let Some(name) = entity.new_names.first().cloned() else { continue };
            let to = self.path_in(&name)?;
            self.touch(name.0);
            if made {
                self.put(&Command::MkDir { path: to, ino: id })?;
            } else {
                let from = self.path(id)?;
                self.put(&Command::Rename { path: from, to })?;
            }
            self.place.insert(id, Place::At(name.0, name.1));
        }
        Ok(())
    }

    /// Step 3: the files that keep their number into place.
    fn files_into_place(&mut self) -> io::Result<()> {
        let ids: Vec<u64> = self.entities.keys().copied().collect();
        for id in ids {
            let Some(entity) = self.entities.get(&id) else { continue };
            if entity.is_dir() || !entity.same() {
                continue;
            }
            let (added, kept) = (entity.added(), entity.kept());
            let Some(first) = added.first() else { continue };
            let target = if let Some(kept) = kept.first() {
                self.path_in(kept)?
            } else {
                let from = self.path(id)?;
                let to = self.path_in(first)?;
                self.put(&Command::Rename { path: from, to: to.clone() })?;
                self.touch(first.0);
                self.place.remove(&id);
                to
            };
            let links = if kept.is_empty() { &added[1..] } else { &added[..] };
            for name in links {
                let path = self.path_in(name)?;
                self.put(&Command::Link { path, target: target.clone() })?;
                self.touch(name.0);
            }
        }
        Ok(())
    }

    /// Step 4: every file's contents, from a second walk.
    fn contents(&mut self) -> io::Result<()> {
        let mut walk = Walk::new(Trees::Both, Key::new(TOP, 0, 0), last_key());
        let mut file: Option<File> = None;
        while let Some(item) = walk.next(self.source)? {
            let ino = item.key.objectid;
            if file.as_ref().map(|f| f.ino) != Some(ino) {
                if let Some(f) = file.take() {
                    self.finish(f)?;
                }
                let replaced = self.replaced.contains(&ino);
                let mut f = self.start(ino);
                if replaced && !f.gone {
                    // The new file of a number used again: all of its
                    // items, whatever the differences say.
                    let to = Key::new(ino, u8::MAX, u64::MAX);
                    let mut whole = Walk::new(Trees::New, Key::new(ino, 0, 0), to);
                    while let Some(item) = whole.next(self.source)? {
                        self.take(&mut f, item)?;
                    }
                    f.inode_changed = true;
                    self.finish(f)?;
                    file = Some(File { ino, gone: true, begun: true, ..File::default() });
                    continue;
                }
                file = Some(f);
            }
            if let Some(f) = file.as_mut() {
                self.take(f, item)?;
            }
        }
        if let Some(f) = file.take() {
            self.finish(f)?;
        }
        // The folders' times, now that what they hold is in place.
        let mut dirs: BTreeSet<u64> = std::mem::take(&mut self.touched);
        dirs.extend(self.dir_times.keys().copied());
        for dir in dirs {
            let times = match self.dir_times.get(&dir) {
                Some(times) => *times,
                None => match self.inode(Trees::New, dir)? {
                    Some(inode) => (inode.atime, inode.mtime, inode.ctime),
                    None => continue,
                },
            };
            let path = self.path(dir)?;
            self.put(&Command::Utimes { path, atime: times.0, mtime: times.1, ctime: times.2 })?;
        }
        Ok(())
    }

    /// File `ino` of step 4, before its items.
    fn start(&self, ino: u64) -> File {
        let mut f = File { ino, ..File::default() };
        if let Some(entity) = self.entities.get(&ino) {
            f.gone = entity.new.is_none();
            f.made = entity.old.is_none();
            (f.old, f.new) = (entity.old, entity.new);
        }
        f
    }

    /// One of file `f`'s items.
    fn take(&mut self, f: &mut File, item: Item) -> io::Result<()> {
        if f.gone {
            return Ok(());
        }
        let Item { key, old, new } = item;
        match key.kind {
            item::INODE_ITEM => {
                if !self.entities.contains_key(&f.ino) {
                    f.made = old.is_none();
                    f.gone = new.is_none();
                }
                if !f.made {
                    f.old = old.as_deref().map(Inode::parse).transpose()?;
                }
                f.new = new.as_deref().map(Inode::parse).transpose()?;
                f.inode_changed = true;
                return Ok(());
            }
            item::INODE_REF | item::INODE_EXTREF => {
                if f.made
                    && !self.entities.contains_key(&f.ino)
                    && let Some(data) = &new
                {
                    items::names(key.kind, key.offset, data, &mut f.names)?;
                }
                return Ok(());
            }
            _ => {}
        }
        if !f.begun {
            self.begin(f)?;
            if f.gone {
                return Ok(());
            }
        }
        let new_inode = f.new.unwrap_or_default();
        match key.kind {
            item::XATTR_ITEM => {
                f.contents_changed = true;
                let old = match (&old, f.made) {
                    (Some(data), false) => items::xattrs(data)?,
                    _ => Vec::new(),
                };
                let new = match &new {
                    Some(data) => items::xattrs(data)?,
                    None => Vec::new(),
                };
                for (name, data) in &new {
                    if !old.iter().any(|(n, d)| n == name && d == data) {
                        let (path, name, data) = (f.path.clone(), name.clone(), data.clone());
                        self.put(&Command::SetXattr { path, name, data })?;
                    }
                }
                for (name, _) in &old {
                    if !new.iter().any(|(n, _)| n == name) {
                        let (path, name) = (f.path.clone(), name.clone());
                        self.put(&Command::RemoveXattr { path, name })?;
                    }
                }
            }
            item::EXTENT_DATA if new_inode.mode & kind::MASK == kind::FILE => {
                f.contents_changed = true;
                let mut ranges = [None, None];
                if let (Some(data), false) = (&old, f.made) {
                    let extent = Extent::parse(data)?;
                    ranges[0] = Some((key.offset, key.offset.saturating_add(extent.len)));
                }
                if let Some(data) = &new {
                    let extent = Extent::parse(data)?;
                    // A new file holds zeros where nothing is written.
                    if !(f.made && extent.empty) {
                        ranges[1] = Some((key.offset, key.offset.saturating_add(extent.len)));
                    }
                }
                for range in ranges.into_iter().flatten() {
                    match f.pending {
                        Some((start, end)) if range.0 <= end => {
                            f.pending = Some((start, end.max(range.1)));
                        }
                        pending => {
                            if let Some(pending) = pending {
                                self.write(f, pending)?;
                            }
                            f.pending = Some(range);
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// File `f` made, if it is new, and its path found: what its items
    /// past its names need.
    fn begin(&mut self, f: &mut File) -> io::Result<()> {
        f.begun = true;
        if f.new.is_none() {
            f.new = self.inode(Trees::New, f.ino)?;
            if f.new.is_none() {
                f.gone = true;
                return Ok(());
            }
        }
        if !f.made && f.old.is_none() {
            f.old = f.new;
        }
        let new = f.new.unwrap_or_default();
        if f.ino == TOP {
            return Ok(());
        }
        let names = match self.entities.get(&f.ino) {
            Some(entity) => entity.new_names.clone(),
            None if f.made => std::mem::take(&mut f.names),
            None => self.new_location(f.ino)?.into_iter().collect(),
        };
        let Some(first) = names.first() else {
            // A file with no name: removed while open, and gone once closed.
            f.gone = true;
            return Ok(());
        };
        f.path = self.path_in(first)?;
        f.size = if f.made { 0 } else { f.old.map_or(0, |inode| inode.size) };
        if !f.made || new.is_dir() {
            return Ok(());
        }
        let (path, ino, mode, rdev) =
            (f.path.clone(), f.ino, u64::from(new.mode), items::stream_rdev(new.rdev));
        let command = match new.mode & kind::MASK {
            kind::FILE => Command::MkFile { path, ino },
            kind::LINK => {
                let target = self.source.read_link(&path)?;
                Command::Symlink { path, ino, target }
            }
            kind::CHAR | kind::BLOCK => Command::MkNod { path, ino, mode, rdev },
            kind::FIFO => Command::MkFifo { path, ino, mode, rdev },
            kind::SOCKET => Command::MkSock { path, ino, mode, rdev },
            _ => return Err(corrupt("inode (of no kind)")),
        };
        self.put(&command)?;
        self.touch(first.0);
        for name in &names[1..] {
            let path = self.path_in(name)?;
            self.put(&Command::Link { path, target: f.path.clone() })?;
            self.touch(name.0);
        }
        Ok(())
    }

    /// Writes the bytes of file `f` from `start` to `end`, as the snapshot
    /// holds them.
    fn write(&mut self, f: &mut File, (start, end): (u64, u64)) -> io::Result<()> {
        let size = f.new.map_or(0, |inode| inode.size);
        let end = end.min(size);
        let mut at = start;
        while at < end {
            let want = (end - at).min(MAX_WRITE as u64) as usize;
            let got = self.source.read(&f.path, at, &mut self.buffer[..want])?.min(want);
            if got == 0 {
                break;
            }
            let data = self.buffer[..got].to_vec();
            self.put(&Command::Write { path: f.path.clone(), offset: at, data })?;
            self.sent.data += got as u64;
            at += got as u64;
            f.size = f.size.max(at);
        }
        Ok(())
    }

    /// File `f` after its items: its size, owner, permissions and times.
    fn finish(&mut self, mut f: File) -> io::Result<()> {
        // Only its names changed, and steps 1 to 3 took care of them.
        if !f.begun && !f.made && !f.inode_changed {
            return Ok(());
        }
        if !f.begun {
            self.begin(&mut f)?;
        }
        if f.gone {
            return Ok(());
        }
        if let Some(pending) = f.pending.take() {
            self.write(&mut f, pending)?;
        }
        let Some(new) = f.new else { return Ok(()) };
        let old = if f.made { None } else { f.old };
        let path = f.path.clone();
        if new.mode & kind::MASK == kind::FILE && f.size != new.size {
            self.put(&Command::Truncate { path: path.clone(), size: new.size })?;
        }
        let chowned = old.is_none_or(|old| old.uid != new.uid || old.gid != new.gid);
        if chowned {
            let (uid, gid) = (u64::from(new.uid), u64::from(new.gid));
            self.put(&Command::Chown { path: path.clone(), uid, gid })?;
        }
        // A change of owner takes the set-user-ID and set-group-ID bits
        // away on Linux, so a file that has them has its mode set again.
        let set_ids = new.mode & 0o6000 != 0;
        if !new.is_link() && (old.is_none_or(|old| old.mode != new.mode) || chowned && set_ids) {
            self.put(&Command::Chmod { path: path.clone(), mode: u64::from(new.mode & 0o7777) })?;
        }
        if new.is_dir() {
            if f.made || f.inode_changed {
                self.dir_times.insert(f.ino, (new.atime, new.mtime, new.ctime));
            }
        } else if f.made || f.inode_changed || f.contents_changed {
            self.put(&Command::Utimes {
                path,
                atime: new.atime,
                mtime: new.mtime,
                ctime: new.ctime,
            })?;
        }
        Ok(())
    }
}

/// `parts`, from the last up, joined by slashes.
fn join(parts: Vec<Vec<u8>>) -> Vec<u8> {
    let mut path = Vec::new();
    for part in parts.into_iter().rev() {
        if !path.is_empty() {
            path.push(b'/');
        }
        path.extend_from_slice(&part);
    }
    path
}
