// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Watches of files and directories (ADR 0615), Linux's inotify: what
//! changes to a node reach the watch files that watch it.
//!
//! A watch is of a node, which it holds as Linux's pins the inode, so a
//! watched node keeps its address while it is watched, and the address
//! is the key the watches are found by: a filesystem has one node for a
//! file while any reference to it lives (`radix_vfs::address`). A table
//! of all the watches, in buckets of entries sorted by key, takes a
//! change's node and its directory and finds the watches to tell. A
//! change that reaches one is built by the hook that made it
//! (`crate::user::notices`) and given to [`tell`], which finds the
//! watches, and queues the event in each one's watch file
//! (`super::watch_file`).
//!
//! [`WATCHED`] counts the watches, so that every hook costs one load
//! while nothing is watched. A hook runs after its change has been made,
//! with no lock held of the filesystem's or the capability store's.
//!
//! Locks: the table's is a spin lock, taken to count and to collect the
//! watches a change reaches. Memory is allocated, events queued and
//! threads woken with it let go: [`tell`] builds the event first,
//! collects the watch files to tell under the lock, and tells them once
//! it is let go, so no other lock is ever taken while the table's is
//! held, and a watch file's queue lock is only ever taken alone. When
//! events are missing, a program's read of its watch file shows what it
//! holds, and `watch_add` of the same file again shows its number.

use alloc::sync::Weak;
use alloc::vec::Vec;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_linux::notify::{IN_ALL_EVENTS, IN_EXCL_UNLINK, IN_IGNORED, IN_ISDIR, IN_ONESHOT};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::SpinLock;
use radix_vfs::{KArc, Kind, NodeRef, mem};

use super::FileObject;
use super::watch_file::{Inbox, Name};
use crate::user::account::{Charge, WatchSlot};

/// How many watches there are, of every watch file. A hook looks at it
/// first: it does nothing while it is 0.
static WATCHED: AtomicUsize = AtomicUsize::new(0);

/// Whether any node is watched.
#[inline]
pub fn watched() -> bool {
    WATCHED.load(Ordering::Relaxed) != 0
}

/// A watch: one watch file's watch of one node.
pub struct Watch {
    /// The address of the node it is of, while the table holds it.
    key: usize,
    /// Its number in its watch file, the `wd` of its events.
    pub number: u32,
    /// The events it asks for, and `IN_ONESHOT` and `IN_EXCL_UNLINK` if
    /// it has them: Linux's bits.
    mask: AtomicU32,
    /// Whether it has been taken out of the table: it tells nothing
    /// more, and its number is no longer one a program may use.
    gone: AtomicBool,
    /// The watch file it is in, which may be going: the table does not
    /// keep it.
    inbox: Weak<Inbox, Kmalloc>,
}

impl Watch {
    /// The events it asks for and its flags.
    pub fn mask(&self) -> u32 {
        self.mask.load(Ordering::Relaxed)
    }

    /// Makes `mask` its events and flags.
    pub fn set_mask(&self, mask: u32) {
        self.mask.store(mask, Ordering::Relaxed);
    }

    /// Whether it has been taken out of the table.
    pub fn is_gone(&self) -> bool {
        self.gone.load(Ordering::Acquire)
    }
}

/// What the table holds of a watch: the watch, its node, which stays in
/// place while it is held, and what the watch is charged. Dropping it
/// ends the watch's hold on the node and gives its charge back.
pub struct Entry {
    key: usize,
    watch: KArc<Watch>,
    _node: NodeRef,
    _charge: Charge,
    _slot: WatchSlot,
}

impl Entry {
    /// The bytes a watch is charged to its account, besides its place
    /// among the account's watches.
    pub const BYTES: u64 = (size_of::<Entry>() + size_of::<Watch>()) as u64;
}

impl Drop for Entry {
    fn drop(&mut self) {
        WATCHED.fetch_sub(1, Ordering::Relaxed);
    }
}

/// How many buckets the table has.
const BUCKETS: usize = 64;

/// The entries whose keys fall in one bucket, sorted by key.
type Bucket = Vec<Entry, Kmalloc>;

/// Every watch. Taken with no other lock held, and never while memory
/// is allocated or an event is queued.
static TABLE: SpinLock<[Bucket; BUCKETS]> =
    SpinLock::new([const { Vec::new_in(HEAP.with(Flags::MAY_SLEEP)) }; BUCKETS]);

fn heap() -> Kmalloc {
    HEAP.with(Flags::MAY_SLEEP)
}

fn no_memory() -> Status {
    Kernel::NoMemory.status(0)
}

/// The bucket of `key`: nodes are made from slabs, so their addresses
/// share their low bits, which are mixed in with higher ones.
fn bucket_of(key: usize) -> usize {
    ((key >> 5) ^ (key >> 11)) % BUCKETS
}

/// The key of `node`, for as long as a watch holds it.
fn key_of(node: &NodeRef) -> usize {
    radix_vfs::address(node)
}

/// Where the entries of `key` start in `bucket`.
fn first_of(bucket: &Bucket, key: usize) -> usize {
    bucket.partition_point(|entry| entry.key < key)
}

/// A watch of `node` for watch file `inbox`, with number `number` and
/// `mask`, charged as `charge` and `slot` say: it is in the table, and
/// holds `node`, until [`remove`] takes it out or an event ends it.
///
/// # Errors
///
/// no_memory if there is no room for it.
pub fn add(
    inbox: Weak<Inbox, Kmalloc>,
    number: u32,
    mask: u32,
    node: &NodeRef,
    charge: Charge,
    slot: WatchSlot,
) -> Result<KArc<Watch>, Status> {
    let key = key_of(node);
    let watch = mem::arc(Watch {
        key,
        number,
        mask: AtomicU32::new(mask),
        gone: AtomicBool::new(false),
        inbox,
    })
    .map_err(|_| no_memory())?;
    WATCHED.fetch_add(1, Ordering::Relaxed);
    let entry =
        Entry { key, watch: watch.clone(), _node: node.clone(), _charge: charge, _slot: slot };
    insert(entry)?;
    Ok(watch)
}

/// Puts `entry` in its bucket, in order of key after those of the same
/// key. Room for it is made with the lock let go, as memory is never
/// allocated under a spin lock, and the entries move to it under the
/// lock: a bucket that has grown does not allocate again for as many.
fn insert(entry: Entry) -> Result<(), Status> {
    let index = bucket_of(entry.key);
    let mut room: Bucket = Vec::new_in(heap());
    let mut entry = Some(entry);
    loop {
        let want = {
            let mut table = TABLE.lock();
            let bucket = &mut table[index];
            if bucket.len() == bucket.capacity() && room.capacity() > bucket.len() {
                // `room` holds more than the bucket did; the order is kept.
                // The bucket's old memory goes to `room`, to be freed once
                // the lock is let go.
                room.clear();
                while let Some(entry) = bucket.pop() {
                    let _ = room.push_within_capacity(entry);
                }
                room.reverse();
                core::mem::swap(bucket, &mut room);
            }
            if bucket.len() < bucket.capacity()
                && let Some(entry) = entry.take()
            {
                let at = bucket.partition_point(|held| held.key <= entry.key);
                // Room was checked just above.
                let _ = bucket.push_within_capacity(entry);
                bucket[at..].rotate_right(1);
                return Ok(());
            }
            bucket.len().saturating_mul(2).max(4)
        };
        room = Vec::new_in(heap());
        room.try_reserve_exact(want).map_err(|_| no_memory())?;
    }
}

/// The watch of `node` that watch file `inbox` has, if it has one.
pub fn find(node: &NodeRef, inbox: &Weak<Inbox, Kmalloc>) -> Option<KArc<Watch>> {
    let key = key_of(node);
    let table = TABLE.lock();
    let bucket = &table[bucket_of(key)];
    bucket[first_of(bucket, key)..]
        .iter()
        .take_while(|entry| entry.key == key)
        .find(|entry| Weak::ptr_eq(&entry.watch.inbox, inbox))
        .map(|entry| entry.watch.clone())
}

/// Takes `watch` out of the table, if it is there: what it held is given
/// back when the entry is dropped, which the caller does with no lock
/// held, as a node's last reference may be the watch's.
pub fn remove(watch: &KArc<Watch>) -> Option<Entry> {
    let mut table = TABLE.lock();
    let bucket = &mut table[bucket_of(watch.key)];
    let at = first_of(bucket, watch.key)
        + bucket[first_of(bucket, watch.key)..]
            .iter()
            .take_while(|entry| entry.key == watch.key)
            .position(|entry| KArc::ptr_eq(&entry.watch, watch))?;
    let entry = bucket.remove(at);
    entry.watch.gone.store(true, Ordering::Release);
    Some(entry)
}

/// What a change tells the watches of the nodes it concerns.
pub struct Notice<'a> {
    /// The event, one of Linux's `IN_` bits, with `IN_ISDIR` if the
    /// change is of a directory.
    pub bits: u32,
    /// Ties the two events of a move together: 0 for any other.
    pub cookie: u32,
    /// The name of the entry the change made, removed or moved, which
    /// the watches of its directory get, as Linux's `fsnotify_parent`
    /// and `fsnotify_name` give it.
    pub name: Option<&'a [u8]>,
    /// Whether the change is of the node itself, seen from its directory
    /// too (a read, a write, an open), rather than of the directory's
    /// entries: `IN_EXCL_UNLINK` keeps the first from a node no directory
    /// holds now.
    pub of_node: bool,
    /// Whether the node is gone: its watches get `IN_IGNORED` after
    /// the event, and end.
    pub ends: bool,
}

/// A watch file to tell, and what: collected under the table's lock.
struct Told {
    inbox: KArc<Inbox>,
    number: u32,
    /// The event for this watch, 0 if it only ends.
    bits: u32,
    /// Whether the event has the entry's name: the watch is of the
    /// directory, not of the node itself.
    named: bool,
    /// Whether the watch has ended, and its watch file gets `IN_IGNORED`.
    ended: bool,
    /// What the table held of the watch, if it ended: dropped once the
    /// lock is let go, as it gives back a charge and lets go of a node.
    entry: Option<Entry>,
}

/// What `watch` of the node itself (`named` false) or of its directory
/// does with `notice`: whether it hears it, and whether it ends.
fn hears(watch: &Watch, notice: &Notice<'_>, named: bool, unlinked: bool) -> (bool, bool) {
    let mask = watch.mask();
    let excluded = named && notice.of_node && unlinked && mask & IN_EXCL_UNLINK != 0;
    let hears = mask & notice.bits & IN_ALL_EVENTS != 0 && !excluded;
    let ends = (hears && mask & IN_ONESHOT != 0) || (notice.ends && !named);
    (hears, ends)
}

/// How many watches `notice` reaches, of the node and of its directory
/// (`keys`), and whether one of the directory's has `IN_EXCL_UNLINK` and
/// hears it.
fn reached(keys: [Option<usize>; 2], notice: &Notice<'_>) -> (usize, bool) {
    let table = TABLE.lock();
    let (mut count, mut excludes) = (0, false);
    for (named, key) in [(false, keys[0]), (true, keys[1])] {
        let Some(key) = key else { continue };
        let bucket = &table[bucket_of(key)];
        let watches = bucket[first_of(bucket, key)..].iter().take_while(|entry| entry.key == key);
        for entry in watches {
            let (hears, ends) = hears(&entry.watch, notice, named, false);
            count += usize::from(hears || ends);
            excludes |= named && hears && entry.watch.mask() & IN_EXCL_UNLINK != 0;
        }
    }
    (count, excludes)
}

/// Collects the watch files `notice` reaches into `told`, which has room
/// for as many as [`reached`] counted, and takes out of the table the
/// watches it ends.
fn collect(
    keys: [Option<usize>; 2],
    notice: &Notice<'_>,
    unlinked: bool,
    told: &mut Vec<Told, Kmalloc>,
) {
    let mut table = TABLE.lock();
    for (named, key) in [(false, keys[0]), (true, keys[1])] {
        let Some(key) = key else { continue };
        let bucket = &mut table[bucket_of(key)];
        let mut at = first_of(bucket, key);
        while at < bucket.len() && bucket[at].key == key {
            let watch = &bucket[at].watch;
            let (hears, ends) = hears(watch, notice, named, unlinked);
            // A watch made since [`reached`] counted was made after the
            // change began. Nothing taken under the lock is dropped under
            // it: the last reference to a watch file may be the one taken
            // here.
            if !(hears || ends) || told.len() == told.capacity() {
                at += 1;
                continue;
            }
            // A watch file that is going has nothing to tell.
            let Some(inbox) = watch.inbox.upgrade() else {
                at += 1;
                continue;
            };
            let bits = if hears { notice.bits } else { 0 };
            let number = watch.number;
            let entry = if ends {
                watch.gone.store(true, Ordering::Release);
                Some(bucket.remove(at))
            } else {
                at += 1;
                None
            };
            let one = Told { inbox, number, bits, named, ended: ends, entry };
            // Room was checked above.
            let _ = told.push_within_capacity(one);
        }
    }
}

/// Tells the watches of `node` and those of `parent`, the directory it
/// is in, what `notice` says changed. The watches of the node hear the
/// event as it is, those of the directory with the entry's name.
/// `node` and `parent` are held by the caller, so a watch's hold on
/// either is never the last, and a watch that ends can let go of its
/// hold here.
///
/// A change that cannot get memory for its event is lost to the watches.
pub fn tell(node: Option<&NodeRef>, parent: Option<&NodeRef>, notice: &Notice<'_>) {
    if !watched() {
        return;
    }
    let keys = [node.map(key_of), parent.map(key_of)];
    let (count, excludes) = reached(keys, notice);
    if count == 0 {
        return;
    }
    // Looked at with no lock held: a filesystem may take its own to say.
    let unlinked = excludes && node.is_some_and(|node| node.stat().links == 0);
    let Ok(name) = notice.name.map(named).transpose() else { return };
    let mut told = Vec::new_in(heap());
    if told.try_reserve_exact(count).is_err() {
        return;
    }
    collect(keys, notice, unlinked, &mut told);
    for one in told {
        // An ended watch's entry is let go here, with no lock held.
        drop(one.entry);
        let name = if one.named { name.clone() } else { None };
        if one.bits != 0 {
            one.inbox.queue(one.number, one.bits, notice.cookie, name);
        }
        if one.ended {
            one.inbox.queue(one.number, IN_IGNORED, 0, None);
        }
        one.inbox.changed();
    }
}

/// Tells the watches of the node `file` is of, and those of the
/// directory it was opened in, with its name, that `bits` happened to it.
#[inline(never)]
pub fn of_file(file: &FileObject, bits: u32) {
    if !watched() {
        return;
    }
    let node = &file.at.node;
    let directory = if node.kind() == Kind::Directory { IN_ISDIR } else { 0 };
    let notice = Notice {
        bits: bits | directory,
        cookie: 0,
        name: file.last_name(),
        of_node: true,
        ends: false,
    };
    tell(Some(node), file.parent(), &notice);
}

/// The name of an entry, in memory its events share.
fn named(bytes: &[u8]) -> Result<Name, Status> {
    let mut copy = Vec::new_in(heap());
    radix_mm::bytes::extend(&mut copy, bytes).map_err(|_| no_memory())?;
    mem::arc(copy).map_err(|_| no_memory())
}

/// A cookie for the two events of a move: never 0, which means none.
pub fn cookie() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed).wrapping_add(1).max(1)
}
