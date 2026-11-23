// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the methods of directories tell the watches of files (ADR 0615):
//! the entries they make, remove and move, and the changes to what a path
//! names. `super::files` calls these, once a change has been made, only
//! while some node is watched (`crate::fs::notify::watched`); the
//! changes of files that are open (a read, a write, a close) are told by
//! [`crate::fs::notify::of_file`], which knows the directory the file was
//! opened in.
//!
//! A change that removes a name must know what the name led to before it
//! goes, so [`before_open`], [`before_remove`] and [`before_rename`] look
//! first and hand what they found to the call that tells afterwards. They
//! resolve the paths a second time, as the directory method does: a
//! second walk, only while something is watched. What they find is the
//! node the last name of the path leads to, without following a link, and
//! its directory; an open or a change through a link names the link in
//! its own directory, not the file the link leads to.
//!
//! When a watch hears too little, `watch_add` of the path again shows
//! whether the watch is of the node the path leads to, and the posix
//! boot test's `inotify` program covers each event.

use alloc::boxed::Box;
use alloc::vec::Vec;

use radix_linux::notify::{
    IN_ATTRIB, IN_CREATE, IN_DELETE, IN_DELETE_SELF, IN_ISDIR, IN_MODIFY, IN_MOVE_SELF,
    IN_MOVED_FROM, IN_MOVED_TO, IN_OPEN,
};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_vfs::mount::{Location, Vfs};
use radix_vfs::{Kind, NodeRef, Who};

use crate::fs::FileRef;
use crate::fs::notify::{self, Notice};

/// A call's finding, in memory of its own, so that a method's frame stays
/// small.
type Found<T> = Box<T, Kmalloc>;

fn found<T>(value: T) -> Option<Found<T>> {
    Box::try_new_in(value, HEAP.with(Flags::MAY_SLEEP)).ok()
}

/// A name in a directory.
struct Entry {
    directory: NodeRef,
    name: Vec<u8, Kmalloc>,
}

impl Entry {
    /// The directory `path`'s last name is in, and the name, as the
    /// methods that make or remove an entry find them; none if `path`
    /// has none.
    fn of(vfs: &Vfs, who: &Who, base: &Location, path: &str) -> Option<Entry> {
        let (directory, name) = vfs.resolve_parent(who, base, path).ok()?;
        Entry::in_directory(directory.node, name)
    }

    fn in_directory(directory: NodeRef, name: &str) -> Option<Entry> {
        let mut bytes = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
        radix_mm::bytes::extend(&mut bytes, name.as_bytes()).ok()?;
        Some(Entry { directory, name: bytes })
    }

    /// Tells the watches of its directory that `bits` happened to the
    /// entry, with `cookie`: the entry's name goes with it.
    fn tell(&self, bits: u32, cookie: u32) {
        let notice = Notice { bits, cookie, name: Some(&self.name), of_node: false, ends: false };
        notify::tell(None, Some(&self.directory), &notice);
    }
}

/// `IN_ISDIR`, if `node` is a directory.
fn directory_bit(node: &NodeRef) -> u32 {
    if node.kind() == Kind::Directory { IN_ISDIR } else { 0 }
}

/// Tells the watches of `node` that `bits` happened to it, and, if it
/// has an entry, those of its directory with its name; `ends` if the
/// node is gone. `IN_ISDIR` goes with them for a directory, but for
/// `IN_DELETE_SELF` and `IN_MOVE_SELF`, which Linux tells without it.
fn tell_node(node: &NodeRef, entry: Option<&Entry>, bits: u32, ends: bool) {
    let itself = bits & (IN_DELETE_SELF | IN_MOVE_SELF) != 0;
    let notice = Notice {
        bits: if itself { bits } else { bits | directory_bit(node) },
        cookie: 0,
        name: entry.map(|entry| &entry.name[..]),
        of_node: true,
        ends,
    };
    notify::tell(Some(node), entry.map(|entry| &entry.directory), &notice);
}

/// A name that a method made: the entry `path` names, a directory if
/// `directory`. For `mkdir`, `symlink`, `mknod` and a socket's bind.
pub fn made(vfs: &Vfs, who: &Who, base: &Location, path: &str, directory: bool) {
    if let Some(entry) = Entry::of(vfs, who, base, path) {
        entry.tell(IN_CREATE | if directory { IN_ISDIR } else { 0 }, 0);
    }
}

/// A hard link `to` was made to what `from` names: `IN_CREATE` in the
/// directory of the new name, and `IN_ATTRIB` for the file, whose links
/// changed. `follow` is whether `from`'s last name was followed if it is a
/// symbolic link.
pub fn linked(vfs: &Vfs, from: (&Who, &Location, &str), to: (&Who, &Location, &str), follow: bool) {
    if let Some(entry) = Entry::of(vfs, to.0, to.1, to.2) {
        entry.tell(IN_CREATE, 0);
    }
    let (who, base, path) = from;
    let node =
        if follow { vfs.resolve(who, base, path) } else { vfs.resolve_link(who, base, path) };
    if let Ok(at) = node {
        tell_node(&at.node, None, IN_ATTRIB, false);
    }
}

/// What the watches hear about the node at `path` once something about
/// it changed (`bits`: its times, its access or its attributes), which a
/// method found at `at`.
pub fn changed(vfs: &Vfs, who: &Who, base: &Location, path: &str, at: &Location, bits: u32) {
    let entry = Entry::of(vfs, who, base, path);
    tell_node(&at.node, entry.as_ref(), bits, false);
}

/// What an open found before it opened.
pub struct Opening {
    entry: Entry,
    /// Whether the name was there, so that the open made nothing.
    existed: bool,
}

/// Looks at where `path` is before an open, which makes the name if
/// `creates`.
pub fn before_open(
    vfs: &Vfs,
    who: &Who,
    base: &Location,
    path: &str,
    creates: bool,
) -> Option<Found<Opening>> {
    let (directory, name) = vfs.resolve_parent(who, base, path).ok()?;
    let existed = !creates || directory.node.lookup(name).is_ok();
    found(Opening { entry: Entry::in_directory(directory.node, name)?, existed })
}

/// Tells what an open did, as `object` it made: `IN_CREATE` if it made
/// the name, `IN_MODIFY` if it cut a file (`truncated`), and `IN_OPEN`;
/// and notes that its close is to be told too, as one of a description
/// opened `for_writing` or not.
pub fn opened(object: &mut FileRef, opening: &Opening, for_writing: bool, truncated: bool) {
    let Opening { entry, existed } = opening;
    let Some(object) = alloc::sync::Arc::get_mut(object) else { return };
    let node = object.at.node.clone();
    if !*existed {
        entry.tell(IN_CREATE, 0);
    }
    tell_node(&node, Some(entry), IN_OPEN, false);
    if truncated && *existed {
        tell_node(&node, Some(entry), IN_MODIFY, false);
    }
    object.open_told(Some(entry.directory.clone()), for_writing);
}

/// What a removal found before it removed.
pub struct Removal {
    entry: Entry,
    /// What the name leads to, held until the removal is told.
    node: NodeRef,
    /// Whether it was the node's last name: a directory has only one.
    last: bool,
}

/// Looks at what `path` names before `unlink` or `rmdir` removes it.
pub fn before_remove(vfs: &Vfs, who: &Who, base: &Location, path: &str) -> Option<Found<Removal>> {
    let (directory, name) = vfs.resolve_parent(who, base, path).ok()?;
    let node = directory.node.lookup(name).ok()?;
    let stat = node.stat();
    let last = stat.kind == Kind::Directory || stat.links <= 1;
    found(Removal { entry: Entry::in_directory(directory.node, name)?, node, last })
}

/// Tells what the removal did: `IN_DELETE` in the directory; for the node,
/// `IN_ATTRIB` if it is not a directory, its links having changed, and
/// `IN_DELETE_SELF` and then `IN_IGNORED` if that was its last name.
pub fn removed(removal: &Removal) {
    let Removal { entry, node, last } = removal;
    entry.tell(IN_DELETE | directory_bit(node), 0);
    // A name that is not a directory's leaves its links changed, which
    // Linux tells even for the last.
    if node.kind() != Kind::Directory || !*last {
        tell_node(node, None, IN_ATTRIB, false);
    }
    if *last {
        tell_node(node, None, IN_DELETE_SELF, true);
    }
}

/// What a rename found before it moved.
pub struct Renaming {
    from: Entry,
    to: Entry,
    moved: NodeRef,
    /// What the new name led to, and whether that was its last name.
    replaced: Option<(NodeRef, bool)>,
    /// Whether both names are of one node, which `rename` leaves alone.
    same: bool,
}

/// Looks at the two names before `rename` moves one to the other.
pub fn before_rename(
    vfs: &Vfs,
    from: (&Who, &Location, &str),
    to: (&Who, &Location, &str),
) -> Option<Found<Renaming>> {
    let (source, from_name) = vfs.resolve_parent(from.0, from.1, from.2).ok()?;
    let (target, to_name) = vfs.resolve_parent(to.0, to.1, to.2).ok()?;
    let moved = source.node.lookup(from_name).ok()?;
    let replaced = target.node.lookup(to_name).ok();
    let same = replaced
        .as_ref()
        .is_some_and(|node| radix_vfs::address(node) == radix_vfs::address(&moved));
    let replaced = replaced.map(|node| {
        let stat = node.stat();
        let last = stat.kind == Kind::Directory || stat.links <= 1;
        (node, last)
    });
    let from = Entry::in_directory(source.node, from_name)?;
    let to = Entry::in_directory(target.node, to_name)?;
    found(Renaming { from, to, moved, replaced, same })
}

/// Tells what the rename did: `IN_MOVED_FROM` and `IN_MOVED_TO` with one
/// cookie, `IN_MOVE_SELF` for the node, and for a node the new name led
/// to, `IN_DELETE_SELF` and `IN_IGNORED` if that was its last name, or
/// `IN_ATTRIB`.
pub fn renamed(renaming: Found<Renaming>) {
    let Renaming { from, to, moved, replaced, same } = *renaming;
    if same {
        return;
    }
    let cookie = notify::cookie();
    let directory = directory_bit(&moved);
    from.tell(IN_MOVED_FROM | directory, cookie);
    to.tell(IN_MOVED_TO | directory, cookie);
    tell_node(&moved, None, IN_MOVE_SELF, false);
    if let Some((node, last)) = replaced {
        if last {
            tell_node(&node, None, IN_DELETE_SELF, true);
        } else {
            tell_node(&node, None, IN_ATTRIB, false);
        }
    }
}
