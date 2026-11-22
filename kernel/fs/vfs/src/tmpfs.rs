// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! tmpfs: a filesystem in memory.
//!
//! Directories list their entries in the order they were made, kept in
//! pieces of at most 112 so that a large directory needs no large
//! allocation, with room that grows as a piece fills so that a small one
//! takes little memory, and find them by name with a map of names that
//! lookups also read without the directory's lock ([`RcuMap`], ADR 0367;
//! it holds its nodes weakly, so that a removed file's space is free at
//! once); files are page caches with no disk below ([`FileCache`] with a
//! non-persistent backing), so a file's pages are its data and holes read
//! as zeros. A filesystem has a size limit, charged by the bytes its files
//! are long, and its files' memory goes when the last reference to them
//! does: a file removed while a program has it open lives until the
//! program lets go, as on Linux.
//!
//! A node keeps its extended attributes in memory too, charged to the
//! limit by their names' and values' bytes, at most [`ATTRIBUTES_MAX`]
//! for one node (ADR 0256).
//!
//! The root filesystem of every Radix system is a tmpfs (spec 7.1), as
//! are `/system/settings` and `/system/state` on top of the read-only boot
//! archive until Phase 10 brings Btrfs.

use alloc::sync::Weak;
use core::any::Any;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use radix_sync::Mutex;
use radix_sync::rcu_map::RcuMap;

use crate::acl::{self, Acl, AclCell};
use crate::cache::{Backing, FileCache};
use crate::mem::{self, Alloc, KArc, KVec};
use crate::path::check_name;
use crate::{
    Access, AccessChange, Error, FileSystem, FsInfo, FsKind, In, Kind, ListFn, Node, NodeRef, Out,
    Result, Setting, SocketBinding, Stat, now,
};

/// The most bytes one node's extended attributes may take, their names'
/// and values' together: room for the longest value Linux allows and
/// some more.
pub const ATTRIBUTES_MAX: u64 = 128 << 10;

/// What every node of one tmpfs shares.
struct Shared {
    /// The most bytes its files may hold together.
    max: u64,
    /// The bytes they hold.
    used: AtomicU64,
    /// The most files and directories it may hold, its root included.
    max_files: u64,
    /// The files and directories it holds, and the names of files past
    /// their first.
    files: AtomicU64,
    next_id: AtomicU64,
    /// Held while names are made, removed or moved, so that a move sees
    /// both directories unchanged and two moves cannot make a loop. Only
    /// one directory is locked at a time under it.
    namespace: Mutex<()>,
}

impl Shared {
    /// Charges `bytes` more to the limit, or fails with
    /// [`Error::NoSpace`].
    fn charge(&self, bytes: u64) -> Result<()> {
        let used = self.used.fetch_add(bytes, Ordering::Relaxed);
        if used + bytes > self.max {
            self.used.fetch_sub(bytes, Ordering::Relaxed);
            return Err(Error::NoSpace);
        }
        Ok(())
    }

    /// Counts one more file, or name, against the limit.
    fn count_file(&self) -> Result<()> {
        self.files
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |files| {
                (files < self.max_files).then_some(files + 1)
            })
            .map(drop)
            .map_err(|_| Error::NoSpace)
    }
}

/// A tmpfs.
pub struct Tmpfs {
    root: KArc<TmpNode>,
    shared: KArc<Shared>,
}

struct Entry {
    /// When it was made, in its directory: a listing goes in this order.
    seq: u64,
    name: KVec<u8>,
    node: KArc<TmpNode>,
}

impl Entry {
    fn name(&self) -> &str {
        core::str::from_utf8(&self.name).unwrap_or("")
    }
}

/// How many entries a piece of a directory holds at most: 112 entries of
/// 72 bytes, as the kernel has them, fill a heap object of 8 KiB, the
/// largest a slab serves.
const PIECE: usize = 112;

const _: () = assert!(core::mem::size_of::<Entry>() * PIECE <= 8192);

/// The room a new piece is made with, in entries. A piece's room doubles
/// as it fills, up to [`PIECE`], so that a directory of a few entries
/// takes a few hundred bytes rather than a whole piece's 8 KiB: most
/// directories are small, and each view a program sees is a tmpfs of
/// its own (ADR 0215).
const FIRST_ROOM: usize = 4;

struct DirState {
    /// The entries in the order they were made in, which listings follow,
    /// in pieces of at most [`PIECE`] entries, so that taking an entry out
    /// moves at most a piece's worth. No piece is empty or has room for
    /// more than [`PIECE`], and two pieces side by side hold more than
    /// [`PIECE`] entries together: the pieces have room for at most about
    /// twice the entries, and the list of pieces, the one allocation that
    /// grows with the directory, holds some 3.7 million entries in the
    /// heap's largest block (65,536 pieces of 40 bytes), more than the file
    /// limit gives a tmpfs on a machine of 28 GiB. A listing's cursor is
    /// the sequence number to go on from, so an entry removed while a
    /// program lists the directory never makes it skip another, as POSIX
    /// `readdir` needs (`rm -r` removes as it lists). The directory's
    /// [`Names`] find an entry by its name.
    pieces: KVec<KVec<Entry>>,
    /// The sequence number of the next entry made.
    next_seq: u64,
    modified: u64,
}

impl DirState {
    fn new(modified: u64) -> DirState {
        DirState { pieces: mem::vec(), next_seq: 1, modified }
    }

    fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// The first piece whose entries do not all come before `seq`: the one
    /// that holds the entry `seq`, if any does.
    fn piece_of(&self, seq: u64) -> usize {
        self.pieces.partition_point(|piece| piece.last().is_some_and(|last| last.seq < seq))
    }

    /// Where the entry `seq` is: its piece, and its place in the piece.
    fn locate(&self, seq: u64) -> Result<(usize, usize)> {
        let piece = self.piece_of(seq);
        let entries = self.pieces.get(piece).ok_or(Error::NotFound)?;
        let at = entries.binary_search_by_key(&seq, |entry| entry.seq);
        Ok((piece, at.map_err(|_| Error::NotFound)?))
    }

    /// The entry `seq`.
    fn get(&self, seq: u64) -> Result<&Entry> {
        let (piece, at) = self.locate(seq)?;
        Ok(&self.pieces[piece][at])
    }

    /// The entry `seq`, to change.
    fn get_mut(&mut self, seq: u64) -> Result<&mut Entry> {
        let (piece, at) = self.locate(seq)?;
        Ok(&mut self.pieces[piece][at])
    }

    /// Adds `node` as `name` after every entry, numbered `next_seq`.
    fn push(&mut self, name: KVec<u8>, node: KArc<TmpNode>) -> Result<()> {
        let entry = Entry { seq: self.next_seq, name, node };
        match self.pieces.last_mut() {
            Some(piece) if piece.len() < PIECE => {
                make_room(piece, 1)?;
                piece.push_within_capacity(entry).map_err(|_| Error::NoMemory)?;
            }
            _ => {
                let mut piece = mem::vec();
                make_room(&mut piece, 1)?;
                piece.push_within_capacity(entry).map_err(|_| Error::NoMemory)?;
                mem::push(&mut self.pieces, piece)?;
            }
        }
        self.next_seq += 1;
        Ok(())
    }

    /// Takes out the entry `seq`. Its piece goes if that leaves it empty,
    /// and otherwise joins the piece before or after it if the two fit in
    /// one.
    fn remove(&mut self, seq: u64) -> Result<Entry> {
        let (piece, at) = self.locate(seq)?;
        let entry = self.pieces[piece].remove(at);
        if self.pieces[piece].is_empty() {
            self.pieces.remove(piece);
        } else if piece == 0 || !self.join(piece - 1) {
            self.join(piece);
        }
        Ok(entry)
    }

    /// Moves the entries of the piece after `piece` onto the end of
    /// `piece` and takes the emptied piece out, if the two hold at most
    /// [`PIECE`] entries together; whether it did. Should `piece` need
    /// more room and the heap have none, the two stay as they are, which
    /// costs only memory.
    fn join(&mut self, piece: usize) -> bool {
        let Some(more) = self.pieces.get(piece + 1).map(KVec::len) else {
            return false;
        };
        let first = &mut self.pieces[piece];
        if first.len() + more > PIECE || make_room(first, more).is_err() {
            return false;
        }
        let next = self.pieces.remove(piece + 1);
        let first = &mut self.pieces[piece];
        for entry in next {
            // Room was made above.
            let _ = first.push_within_capacity(entry);
        }
        true
    }
}

/// Makes room in `piece` for `more` entries, which with its own number at
/// most [`PIECE`]: its room at least doubles, from [`FIRST_ROOM`], and
/// stops at [`PIECE`], as a vector's does without the cap.
fn make_room(piece: &mut KVec<Entry>, more: usize) -> Result<()> {
    let needed = piece.len() + more;
    if needed <= piece.capacity() {
        return Ok(());
    }
    let room = needed.max(2 * piece.capacity()).clamp(FIRST_ROOM, PIECE);
    piece.try_reserve_exact(room - piece.len()).map_err(|_| Error::NoMemory)
}

/// What a directory's map of names holds for a name: the entry's sequence
/// number, and its node, held weakly. A removed name's link is freed after
/// an RCU grace period, and a strong reference in it would keep the node,
/// and the space its bytes are charged for, until then. The entries and
/// open files own the nodes.
#[derive(Clone)]
struct Named {
    seq: u64,
    node: Weak<TmpNode, Alloc>,
}

/// A directory's names: changed with its entries, under its lock, where
/// they find an entry by its name, and read without it by lookups.
type Names = RcuMap<Named, Alloc>;

/// The sequence number of the entry `name` in a directory with `names`.
/// Exact under the directory's lock, which every change to its names
/// holds.
fn seq_of(names: &Names, name: &str) -> Option<u64> {
    names.get(name.as_bytes()).map(|named| named.seq)
}

enum Body {
    Dir(Mutex<DirState>, Names),
    File {
        cache: FileCache,
        charged: AtomicU64,
    },
    /// A symbolic link's target, charged to the limit like a file's
    /// bytes, and when it was last changed, as `set_times` sets it.
    Link {
        target: KVec<u8>,
        modified: AtomicU64,
    },
    /// A socket's name, with what the kernel keeps in it
    /// ([`Node::make_socket`]; nothing for one `make_node` made), and
    /// when it was last changed, as `set_times` sets it.
    Socket {
        binding: Option<SocketBinding>,
        modified: AtomicU64,
    },
    /// A named pipe or a device node ([`Node::make_node`]): its kind, the
    /// device number it stands for, and when it was last changed.
    Special {
        kind: Kind,
        device: u64,
        modified: AtomicU64,
    },
}

/// A tmpfs file, directory, symbolic link, socket's name, named pipe or
/// device node.
pub struct TmpNode {
    id: u64,
    created: u64,
    /// When it was last read, as `set_times` set it; reads leave it.
    accessed: AtomicU64,
    /// When a name of it or a time was last changed; its contents'
    /// changes are its modification time, which is later then.
    changed: AtomicU64,
    /// How many names it has.
    links: AtomicU32,
    /// Who it belongs to and its mode (ADR 0213).
    access: Mutex<Access>,
    /// Its extended attributes (ADR 0256).
    attributes: Mutex<Attributes>,
    /// Its access list, as its attribute holds it.
    acl: AclCell,
    shared: KArc<Shared>,
    /// The directory it is in; none for the root, or once it is removed.
    parent: Mutex<Option<Weak<TmpNode, Alloc>>>,
    /// Itself, so that it can be its entries' parent.
    me: radix_sync::Once<Weak<TmpNode, Alloc>>,
    body: Body,
}

impl Tmpfs {
    /// An empty tmpfs whose files may hold `max` bytes together, and
    /// which may hold `max_files` files and directories, its root
    /// included. Its root belongs to user 0 and has mode `root_mode`.
    pub fn new(max: u64, max_files: u64, root_mode: u16) -> Result<KArc<Tmpfs>> {
        let shared = mem::arc(Shared {
            max,
            used: AtomicU64::new(0),
            max_files,
            files: AtomicU64::new(0),
            next_id: AtomicU64::new(2),
            namespace: Mutex::new(()),
        })?;
        let access = Access { owner: 0, group: 0, mode: root_mode & crate::mode::ALL };
        let root = TmpNode::new(1, shared.clone(), Kind::Directory, access)?;
        Ok(mem::arc(Tmpfs { root, shared })?)
    }
}

impl FileSystem for Tmpfs {
    fn kind(&self) -> FsKind {
        FsKind::Tmpfs
    }

    fn root(&self) -> NodeRef {
        self.root.clone()
    }

    fn light_drops(&self) -> bool {
        true
    }

    fn info(&self) -> FsInfo {
        let page = radix_block::PAGE_SIZE as u64;
        FsInfo {
            block_size: page as u32,
            blocks: self.shared.max / page,
            free: self.shared.max.saturating_sub(self.shared.used.load(Ordering::Relaxed)) / page,
            files: self.shared.max_files,
            free_files: self
                .shared
                .max_files
                .saturating_sub(self.shared.files.load(Ordering::Relaxed)),
        }
    }
}

impl TmpNode {
    fn new(id: u64, shared: KArc<Shared>, kind: Kind, access: Access) -> Result<KArc<TmpNode>> {
        let created = now();
        let body = match kind {
            Kind::Directory => {
                Body::Dir(Mutex::new(DirState::new(created)), RcuMap::new_in(mem::alloc()))
            }
            Kind::File => {
                Body::File { cache: FileCache::new(0, created), charged: AtomicU64::new(0) }
            }
            _ => return Err(Error::NotSupported),
        };
        TmpNode::with_body(id, shared, created, body, access)
    }

    /// A symbolic link to `target`.
    fn link(id: u64, shared: KArc<Shared>, target: &str, access: Access) -> Result<KArc<TmpNode>> {
        let bytes = target.len() as u64;
        let used = shared.used.fetch_add(bytes, Ordering::Relaxed);
        if used + bytes > shared.max {
            shared.used.fetch_sub(bytes, Ordering::Relaxed);
            return Err(Error::NoSpace);
        }
        let created = now();
        let body = match mem::copy(target.as_bytes()) {
            Ok(target) => Body::Link { target, modified: AtomicU64::new(created) },
            Err(error) => {
                shared.used.fetch_sub(bytes, Ordering::Relaxed);
                return Err(error.into());
            }
        };
        TmpNode::with_body(id, shared, created, body, access)
    }

    /// A socket's name, holding `binding`.
    fn socket(
        id: u64,
        shared: KArc<Shared>,
        binding: Option<SocketBinding>,
        access: Access,
    ) -> Result<KArc<TmpNode>> {
        let created = now();
        let body = Body::Socket { binding, modified: AtomicU64::new(created) };
        TmpNode::with_body(id, shared, created, body, access)
    }

    /// A node with `body` and `access`; a node that cannot be made hands
    /// back what its body was charged, as one dropped does.
    fn with_body(
        id: u64,
        shared: KArc<Shared>,
        created: u64,
        body: Body,
        access: Access,
    ) -> Result<KArc<TmpNode>> {
        // Each counts against the limit until it is dropped, as with
        // Linux's nr_inodes, so that empty files cannot use up the heap.
        if shared.count_file().is_err() {
            if let Body::Link { target, .. } = &body {
                shared.used.fetch_sub(target.len() as u64, Ordering::Relaxed);
            }
            return Err(Error::NoSpace);
        }
        let node = mem::arc(TmpNode {
            id,
            created,
            accessed: AtomicU64::new(created),
            changed: AtomicU64::new(created),
            links: AtomicU32::new(0),
            access: Mutex::new(access),
            attributes: Mutex::new(Attributes { list: mem::vec(), bytes: 0 }),
            acl: AclCell::none(),
            shared,
            parent: Mutex::new(None),
            me: radix_sync::Once::new(),
            body,
        })?;
        let _ = node.me.set(KArc::downgrade(&node));
        Ok(node)
    }

    /// A weak reference to itself.
    fn weak(&self) -> Option<Weak<TmpNode, Alloc>> {
        self.me.get().cloned()
    }

    fn dir(&self) -> Result<&Mutex<DirState>> {
        match &self.body {
            Body::Dir(dir, _) => Ok(dir),
            _ => Err(Error::NotDirectory),
        }
    }

    /// A directory's names for lookups ([`Names`]): changed with its
    /// entries, under its lock.
    fn names(&self) -> Result<&Names> {
        match &self.body {
            Body::Dir(_, names) => Ok(names),
            _ => Err(Error::NotDirectory),
        }
    }

    fn file(&self) -> Result<(&FileCache, Charge<'_>)> {
        match &self.body {
            Body::File { cache, charged } => Ok((cache, Charge { shared: &self.shared, charged })),
            Body::Dir(..) => Err(Error::IsDirectory),
            // Resolution follows links, so only a caller that asked not
            // to reaches one here; a socket's name cannot be opened, and
            // the kernel opens pipes and devices for their nodes.
            Body::Link { .. } | Body::Socket { .. } | Body::Special { .. } => {
                Err(Error::NotSupported)
            }
        }
    }

    /// Adds `node`, new, to this directory as `name`.
    fn add(&self, name: &str, node: KArc<TmpNode>) -> Result<NodeRef> {
        self.add_name(name, node, false)
    }

    /// Adds `name` for `node`, a new node, or with `again` one more name
    /// of a file that has one. Each name after a file's first counts
    /// against the limit as a file does, as on Linux, since each holds
    /// memory until it is removed.
    fn add_name(&self, name: &str, node: KArc<TmpNode>, again: bool) -> Result<NodeRef> {
        let dir = self.dir()?;
        let names = self.names()?;
        let name_bytes = mem::copy(name.as_bytes())?;
        let _namespace = self.shared.namespace.lock();
        let mut dir = dir.lock();
        if seq_of(names, name).is_some() {
            return Err(Error::Exists);
        }
        if matches!(node.body, Body::Dir(..)) {
            *node.parent.lock() = self.weak();
        }
        if again {
            // A file whose names are all gone stays gone, as on Linux
            // (ENOENT).
            if node.links.load(Ordering::Relaxed) == 0 {
                return Err(Error::NotFound);
            }
            self.shared.count_file()?;
        }
        let named = Named { seq: dir.next_seq, node: KArc::downgrade(&node) };
        let made = names.pending(name.as_bytes(), named).ok_or(Error::NoMemory);
        if let Err(e) = made.and_then(|pending| {
            dir.push(name_bytes, node.clone())?;
            names.insert(pending);
            Ok(())
        }) {
            if again {
                self.shared.files.fetch_sub(1, Ordering::Relaxed);
            }
            return Err(e);
        }
        let time = now();
        dir.modified = time;
        node.named(time);
        Ok(node)
    }

    /// It has gained a name at `time`.
    fn named(&self, time: u64) {
        self.links.fetch_add(1, Ordering::Relaxed);
        self.changed.store(time, Ordering::Relaxed);
    }

    /// It has lost a name at `time`. A name past its first gives back
    /// what it counted ([`TmpNode::add_name`]).
    fn unnamed(&self, time: u64) {
        if self.links.fetch_sub(1, Ordering::Relaxed) > 1 {
            self.shared.files.fetch_sub(1, Ordering::Relaxed);
        }
        self.changed.store(time, Ordering::Relaxed);
    }

    /// Whether it is a directory with nothing in it. Called with its
    /// parent's (or the directory it would replace's) lock held, and the
    /// namespace lock, so only one thread ever takes two directory locks
    /// at once, the outer one first.
    fn is_empty_dir(&self) -> bool {
        matches!(&self.body, Body::Dir(dir, _) if dir.lock_nested(1).is_empty())
    }
}

/// A node's extended attributes: each name with its value, in the order
/// they were made, and the bytes they are charged.
struct Attributes {
    list: KVec<(KVec<u8>, KVec<u8>)>,
    bytes: u64,
}

impl Attributes {
    /// Where `name` is in the list.
    fn find(&self, name: &[u8]) -> Option<usize> {
        self.list.iter().position(|(n, _)| n[..] == *name)
    }
}

/// The backing of a tmpfs file: no disk, and growth charged to the
/// filesystem's limit.
struct Charge<'a> {
    shared: &'a Shared,
    /// The bytes this file is charged.
    charged: &'a AtomicU64,
}

impl Backing for Charge<'_> {
    fn persistent(&self) -> bool {
        false
    }

    fn reserve(&self, size: u64) -> Result<()> {
        let had = self.charged.load(Ordering::Relaxed);
        if size <= had {
            return Ok(());
        }
        let more = size - had;
        let used = self.shared.used.fetch_add(more, Ordering::Relaxed);
        if used + more > self.shared.max {
            self.shared.used.fetch_sub(more, Ordering::Relaxed);
            return Err(Error::NoSpace);
        }
        self.charged.store(size, Ordering::Relaxed);
        Ok(())
    }

    fn truncate(&self, size: u64) -> Result<()> {
        let had = self.charged.load(Ordering::Relaxed);
        if size < had {
            self.shared.used.fetch_sub(had - size, Ordering::Relaxed);
            self.charged.store(size, Ordering::Relaxed);
        }
        Ok(())
    }
}

impl Drop for TmpNode {
    fn drop(&mut self) {
        self.shared.files.fetch_sub(1, Ordering::Relaxed);
        let charged = match &self.body {
            Body::File { charged, .. } => charged.load(Ordering::Relaxed),
            Body::Link { target, .. } => target.len() as u64,
            Body::Dir(..) | Body::Socket { .. } | Body::Special { .. } => 0,
        };
        let attributes = self.attributes.get_mut().bytes;
        self.shared.used.fetch_sub(charged + attributes, Ordering::Relaxed);
    }
}

impl Node for TmpNode {
    fn stat(&self) -> Stat {
        let (size, modified, pages) = match &self.body {
            Body::Dir(dir, _) => (0, dir.lock().modified, 0),
            Body::File { cache, .. } => cache.status(),
            Body::Link { target, modified } => {
                (target.len() as u64, modified.load(Ordering::Relaxed), 0)
            }
            Body::Socket { modified, .. } | Body::Special { modified, .. } => {
                (0, modified.load(Ordering::Relaxed), 0)
            }
        };
        let device = match self.body {
            Body::Special { device, .. } => device,
            _ => 0,
        };
        let allocated = (pages * radix_block::PAGE_SIZE) as u64;
        Stat {
            kind: self.kind(),
            size,
            allocated,
            id: self.id,
            created: self.created,
            modified,
            accessed: self.accessed.load(Ordering::Relaxed),
            changed: self.changed.load(Ordering::Relaxed).max(modified),
            // A directory's count of its subdirectories is not kept: 1,
            // as Btrfs has it, says so to programs that look.
            links: match self.body {
                Body::Dir(..) => 1,
                _ => self.links.load(Ordering::Relaxed),
            },
            device,
        }
    }

    fn kind(&self) -> Kind {
        match self.body {
            Body::Dir(..) => Kind::Directory,
            Body::File { .. } => Kind::File,
            Body::Link { .. } => Kind::Symlink,
            Body::Socket { .. } => Kind::Socket,
            Body::Special { kind, .. } => kind,
        }
    }

    // A socket's name is connected through, never opened (ENXIO).
    fn open(&self) -> Result<Option<NodeRef>> {
        match self.body {
            Body::Socket { .. } => Err(Error::NoDevice),
            _ => Ok(None),
        }
    }

    fn lookup(&self, name: &str) -> Result<NodeRef> {
        // Without the directory's lock: one lookup, in a read section. A
        // node that is gone by the upgrade lost its name after the lookup,
        // as with an unlink that wins the race on Linux.
        let node = self.names()?.get(name.as_bytes()).and_then(|named| named.node.upgrade());
        Ok(node.ok_or(Error::NotFound)?)
    }

    fn access(&self) -> Access {
        *self.access.lock()
    }

    fn set_access(&self, change: &AccessChange) -> Result<()> {
        if change.mode.is_some() && matches!(self.body, Body::Link { .. }) {
            return Err(Error::NotSupported);
        }
        let mut access = self.access.lock();
        *access = change.apply(*access);
        self.changed.store(now(), Ordering::Relaxed);
        Ok(())
    }

    fn create(&self, name: &str, kind: Kind, access: Access) -> Result<NodeRef> {
        check_name(name)?;
        self.dir()?;
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        self.add(name, TmpNode::new(id, self.shared.clone(), kind, access)?)
    }

    fn make_link(&self, name: &str, target: &str, access: Access) -> Result<NodeRef> {
        check_name(name)?;
        self.dir()?;
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        self.add(name, TmpNode::link(id, self.shared.clone(), target, access)?)
    }

    fn make_socket(&self, name: &str, binding: SocketBinding, access: Access) -> Result<NodeRef> {
        check_name(name)?;
        self.dir()?;
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        self.add(name, TmpNode::socket(id, self.shared.clone(), Some(binding), access)?)
    }

    fn link(&self, name: &str, node: &NodeRef) -> Result<()> {
        check_name(name)?;
        self.dir()?;
        let node = crate::downcast::<TmpNode>(&**node).ok_or(Error::CrossMount)?;
        if !KArc::ptr_eq(&node.shared, &self.shared) {
            return Err(Error::CrossMount);
        }
        if matches!(node.body, Body::Dir(..)) {
            return Err(Error::IsDirectory);
        }
        let me = node.weak().and_then(|weak| weak.upgrade()).ok_or(Error::NotFound)?;
        self.add_name(name, me, true).map(drop)
    }

    fn make_node(&self, name: &str, kind: Kind, device: u64, access: Access) -> Result<NodeRef> {
        check_name(name)?;
        self.dir()?;
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let shared = self.shared.clone();
        let node = match kind {
            Kind::File => TmpNode::new(id, shared, kind, access)?,
            Kind::Socket => TmpNode::socket(id, shared, None, access)?,
            Kind::Pipe | Kind::CharDevice | Kind::BlockDevice => {
                let created = now();
                let device = if kind == Kind::Pipe { 0 } else { device };
                let body = Body::Special { kind, device, modified: AtomicU64::new(created) };
                TmpNode::with_body(id, shared, created, body, access)?
            }
            _ => return Err(Error::NotSupported),
        };
        self.add(name, node)
    }

    fn socket_binding(&self) -> Option<SocketBinding> {
        match &self.body {
            Body::Socket { binding, .. } => binding.clone(),
            _ => None,
        }
    }

    fn read_link(&self, out: &mut dyn Out) -> Result<usize> {
        match &self.body {
            Body::Link { target, .. } => {
                let room = out.len().min(target.len());
                out.put(0, &target[..room])?;
                Ok(target.len())
            }
            _ => Err(Error::NotLink),
        }
    }

    fn remove(&self, name: &str) -> Result<()> {
        check_name(name)?;
        let _namespace = self.shared.namespace.lock();
        let mut dir = self.dir()?.lock();
        let names = self.names()?;
        let seq = seq_of(names, name).ok_or(Error::NotFound)?;
        let node = &dir.get(seq)?.node;
        if matches!(node.body, Body::Dir(..)) && !node.is_empty_dir() {
            return Err(Error::NotEmpty);
        }
        *node.parent.lock() = None;
        let time = now();
        node.unnamed(time);
        dir.remove(seq)?;
        names.remove(name.as_bytes());
        dir.modified = time;
        Ok(())
    }

    fn rename(&self, from: &str, to_dir: &dyn Node, to: &str) -> Result<()> {
        check_name(from)?;
        check_name(to)?;
        let target = crate::downcast::<TmpNode>(to_dir).ok_or(Error::CrossMount)?;
        if !KArc::ptr_eq(&target.shared, &self.shared) {
            return Err(Error::CrossMount);
        }
        target.dir()?;
        let _namespace = self.shared.namespace.lock();
        if core::ptr::eq(self, target) {
            return rename_within(&mut self.dir()?.lock(), self.names()?, from, to);
        }
        // The entry to move, checked against what it would replace.
        let moving = {
            let source = self.dir()?.lock();
            let seq = seq_of(self.names()?, from).ok_or(Error::NotFound)?;
            source.get(seq)?.node.clone()
        };
        // A directory cannot move under itself.
        let target_ref = target.weak().and_then(|weak| weak.upgrade()).ok_or(Error::NotFound)?;
        if matches!(moving.body, Body::Dir(..)) && is_under(&target_ref, &moving) {
            return Err(Error::BadName);
        }
        let name = mem::copy(to.as_bytes())?;
        let names = target.names()?;
        {
            let mut dest = target.dir()?.lock();
            // `to` leads to the entry it names, which keeps its place in
            // listings, or to a new one.
            let replaced = seq_of(names, to);
            let named =
                Named { seq: replaced.unwrap_or(dest.next_seq), node: KArc::downgrade(&moving) };
            let pending = names.pending(to.as_bytes(), named).ok_or(Error::NoMemory)?;
            match replaced {
                Some(seq) => {
                    let entry = dest.get_mut(seq)?;
                    // Two names of one file: nothing happens, as POSIX has it.
                    if KArc::ptr_eq(&entry.node, &moving) {
                        return Ok(());
                    }
                    check_replace(&moving, &entry.node)?;
                    *entry.node.parent.lock() = None;
                    entry.node.unnamed(now());
                    entry.node = moving.clone();
                    names.remove(to.as_bytes());
                }
                None => dest.push(name, moving.clone())?,
            }
            names.insert(pending);
            dest.modified = now();
        }
        if matches!(moving.body, Body::Dir(..)) {
            *moving.parent.lock() = target.weak();
        }
        moving.changed.store(now(), Ordering::Relaxed);
        // The namespace lock kept the entry where it was.
        let mut source = self.dir()?.lock();
        let names = self.names()?;
        if let Some(seq) = seq_of(names, from) {
            source.remove(seq)?;
            names.remove(from.as_bytes());
        }
        source.modified = now();
        Ok(())
    }

    fn list(&self, cursor: u64, f: ListFn<'_>) -> Result<()> {
        let dir = self.dir()?.lock();
        let entries = dir.pieces[dir.piece_of(cursor)..].iter().flatten();
        for entry in entries.skip_while(|entry| entry.seq < cursor) {
            if !f(entry.seq + 1, entry.name(), entry.node.kind()) {
                break;
            }
        }
        Ok(())
    }

    fn read(&self, offset: u64, out: &mut dyn Out) -> Result<usize> {
        let (cache, backing) = self.file()?;
        cache.read(offset, out, &backing)
    }

    fn write(&self, offset: u64, data: &dyn In) -> Result<usize> {
        let (cache, backing) = self.file()?;
        cache.write(offset, data, &backing)
    }

    fn set_size(&self, size: u64) -> Result<()> {
        let (cache, backing) = self.file()?;
        cache.set_size(size, &backing)
    }

    fn set_times(&self, accessed: Option<u64>, modified: Option<u64>) -> Result<()> {
        if let Some(accessed) = accessed {
            self.accessed.store(accessed, Ordering::Relaxed);
        }
        if let Some(modified) = modified {
            match &self.body {
                Body::Dir(dir, _) => dir.lock().modified = modified,
                Body::File { cache, .. } => cache.set_modified(modified),
                Body::Link { modified: time, .. }
                | Body::Socket { modified: time, .. }
                | Body::Special { modified: time, .. } => {
                    time.store(modified, Ordering::Relaxed);
                }
            }
        }
        // Setting a time changes the node, now.
        self.changed.store(now(), Ordering::Relaxed);
        Ok(())
    }

    fn get_attribute(&self, name: &[u8]) -> Result<KVec<u8>> {
        let attributes = self.attributes.lock();
        let at = attributes.find(name).ok_or(Error::NoAttribute)?;
        Ok(mem::copy(&attributes.list[at].1)?)
    }

    fn set_attribute(&self, name: &[u8], value: &[u8], how: Setting) -> Result<()> {
        let mut attributes = self.attributes.lock();
        let at = attributes.find(name);
        match (at, how) {
            (Some(_), Setting::Create) => return Err(Error::Exists),
            (None, Setting::Replace) => return Err(Error::NoAttribute),
            _ => {}
        }
        let had = at.map_or(0, |at| {
            let (name, value) = &attributes.list[at];
            (name.len() + value.len()) as u64
        });
        let bytes = (name.len() + value.len()) as u64;
        if attributes.bytes - had + bytes > ATTRIBUTES_MAX {
            return Err(Error::NoSpace);
        }
        let value = mem::copy(value)?;
        let made = match at {
            Some(_) => None,
            None => {
                // Room in the list first, so that nothing fails once the
                // bytes are charged.
                attributes.list.try_reserve(1).map_err(|_| Error::NoMemory)?;
                Some(mem::copy(name)?)
            }
        };
        if bytes > had {
            self.shared.charge(bytes - had)?;
        } else {
            self.shared.used.fetch_sub(had - bytes, Ordering::Relaxed);
        }
        match (at, made) {
            (Some(at), _) => attributes.list[at].1 = value,
            (None, Some(name)) => mem::push(&mut attributes.list, (name, value))?,
            (None, None) => {}
        }
        attributes.bytes = attributes.bytes - had + bytes;
        drop(attributes);
        if name == acl::ACCESS {
            self.acl.forget();
        }
        self.changed.store(now(), Ordering::Relaxed);
        Ok(())
    }

    fn list_attributes(&self) -> Result<KVec<u8>> {
        let attributes = self.attributes.lock();
        let mut names = mem::vec();
        for (name, _) in &attributes.list {
            mem::extend(&mut names, name)?;
            mem::extend(&mut names, &[0])?;
        }
        Ok(names)
    }

    fn remove_attribute(&self, name: &[u8]) -> Result<()> {
        let mut attributes = self.attributes.lock();
        let at = attributes.find(name).ok_or(Error::NoAttribute)?;
        let (name, value) = attributes.list.remove(at);
        let bytes = (name.len() + value.len()) as u64;
        attributes.bytes -= bytes;
        self.shared.used.fetch_sub(bytes, Ordering::Relaxed);
        drop(attributes);
        if name[..] == *acl::ACCESS {
            self.acl.forget();
        }
        self.changed.store(now(), Ordering::Relaxed);
        Ok(())
    }

    fn access_list(&self) -> Result<Option<KArc<Acl>>> {
        self.acl.get(|| self.get_attribute(acl::ACCESS))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Whether `node` is `dir` or lies under it.
fn is_under(node: &KArc<TmpNode>, dir: &KArc<TmpNode>) -> bool {
    let mut current = node.clone();
    loop {
        if KArc::ptr_eq(&current, dir) {
            return true;
        }
        let parent = current.parent.lock().as_ref().and_then(Weak::upgrade);
        match parent {
            Some(parent) => current = parent,
            None => return false,
        }
    }
}

/// Whether `moving` may replace `old`: a file, symbolic link or socket's
/// name replaces any of them, a directory an empty directory.
fn check_replace(moving: &TmpNode, old: &TmpNode) -> Result<()> {
    match (&moving.body, &old.body) {
        (Body::Dir(..), Body::Dir(..)) if !old.is_empty_dir() => Err(Error::NotEmpty),
        (Body::Dir(..), Body::Dir(..)) => Ok(()),
        (Body::Dir(..), _) => Err(Error::NotDirectory),
        (_, Body::Dir(..)) => Err(Error::IsDirectory),
        _ => Ok(()),
    }
}

/// A rename inside one directory. The entry keeps its place in listings
/// unless it replaces another, whose place it takes.
fn rename_within(dir: &mut DirState, names: &Names, from: &str, to: &str) -> Result<()> {
    let from_seq = seq_of(names, from).ok_or(Error::NotFound)?;
    if from == to {
        return Ok(());
    }
    let mut name = mem::copy(to.as_bytes())?;
    let moving = dir.get(from_seq)?.node.clone();
    let replaced = seq_of(names, to);
    let named = Named { seq: replaced.unwrap_or(from_seq), node: KArc::downgrade(&moving) };
    let pending = names.pending(to.as_bytes(), named).ok_or(Error::NoMemory)?;
    match replaced {
        Some(seq) => {
            let entry = dir.get_mut(seq)?;
            // Two names of one file: nothing happens, as POSIX has it.
            if KArc::ptr_eq(&entry.node, &moving) {
                return Ok(());
            }
            check_replace(&moving, &entry.node)?;
            *entry.node.parent.lock() = None;
            entry.node.unnamed(now());
            moving.changed.store(now(), Ordering::Relaxed);
            entry.node = moving;
            dir.remove(from_seq)?;
            names.remove(to.as_bytes());
        }
        None => {
            let entry = dir.get_mut(from_seq)?;
            core::mem::swap(&mut entry.name, &mut name);
            entry.node.changed.store(now(), Ordering::Relaxed);
        }
    }
    // The name now leads where `from` did, and `from` nowhere.
    names.remove(from.as_bytes());
    names.insert(pending);
    dir.modified = now();
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    use proptest::prelude::*;

    use super::*;

    const FILE: Access = Access::default_for(Kind::File);

    #[derive(Clone, Debug)]
    enum Op {
        Make(u16),
        Remove(u16),
        Rename(u16, u16),
    }

    fn name(n: u16) -> String {
        std::format!("f{n}")
    }

    /// The names a listing gives, read one at a time from each cursor.
    fn listed(dir: &TmpNode) -> Vec<String> {
        let mut names = Vec::new();
        let mut cursor = 0;
        loop {
            let mut next = None;
            dir.list(cursor, &mut |at, name, _| {
                next = Some((at, name.to_string()));
                false
            })
            .unwrap();
            let Some((at, name)) = next else { return names };
            names.push(name);
            cursor = at;
        }
    }

    /// The directory's pieces keep their promises: none is empty, each
    /// has room for a whole piece and holds no more, their entries go up
    /// in order, and two side by side hold more than a piece together.
    fn check_pieces(dir: &DirState) -> core::result::Result<(), TestCaseError> {
        let mut last = 0;
        for (i, piece) in dir.pieces.iter().enumerate() {
            prop_assert!(!piece.is_empty() && piece.len() <= PIECE && piece.capacity() <= PIECE);
            for entry in piece {
                prop_assert!(entry.seq > last);
                last = entry.seq;
            }
            if let Some(next) = dir.pieces.get(i + 1) {
                prop_assert!(piece.len() + next.len() > PIECE);
            }
        }
        prop_assert!(last < dir.next_seq);
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        /// A directory of a few pieces against a list of its names in
        /// listing order: a new name goes last, a renamed entry keeps its
        /// place, and one that replaces another takes the other's place.
        #[test]
        fn a_directory_in_pieces_matches_a_model(ops in proptest::collection::vec(prop_oneof![
            4 => (0u16..400).prop_map(Op::Make),
            2 => (0u16..400).prop_map(Op::Remove),
            1 => (0u16..400, 0u16..400).prop_map(|(a, b)| Op::Rename(a, b)),
        ], 1..1500)) {
            let fs = Tmpfs::new(1 << 20, u64::MAX, 0o755).unwrap();
            let root = &fs.root;
            let mut model: Vec<String> = Vec::new();
            let place = |model: &Vec<String>, n: u16| model.iter().position(|it| *it == name(n));
            for op in ops {
                match op {
                    Op::Make(n) => {
                        let made = root.create(&name(n), Kind::File, FILE);
                        prop_assert_eq!(made.is_ok(), place(&model, n).is_none());
                        if made.is_ok() {
                            model.push(name(n));
                        }
                    }
                    Op::Remove(n) => {
                        let removed = root.remove(&name(n));
                        prop_assert_eq!(removed.is_ok(), place(&model, n).is_some());
                        model.retain(|it| *it != name(n));
                    }
                    Op::Rename(a, b) => {
                        let renamed = root.rename(&name(a), &**root, &name(b));
                        prop_assert_eq!(renamed.is_ok(), place(&model, a).is_some());
                        match (place(&model, a), place(&model, b)) {
                            (Some(_), _) if a == b => {}
                            (Some(from), Some(_)) => {
                                model.remove(from);
                            }
                            (Some(from), None) => model[from] = name(b),
                            (None, _) => {}
                        }
                    }
                }
                check_pieces(&root.dir().unwrap().lock())?;
                prop_assert_eq!(listed(root), model.clone());
                for it in &model {
                    prop_assert!(root.lookup(it).is_ok());
                }
            }
        }
    }

    /// Removing every entry of a directory of thousands, each as soon as
    /// it is listed as `rm -r` does, leaves it empty with no piece.
    #[test]
    fn a_large_directory_empties_as_it_is_listed() {
        let fs = Tmpfs::new(1 << 20, u64::MAX, 0o755).unwrap();
        let root = &fs.root;
        for n in 0..5000 {
            root.create(&name(n), Kind::File, FILE).unwrap();
        }
        assert_eq!(root.dir().unwrap().lock().pieces.len(), 5000_usize.div_ceil(PIECE));
        let mut cursor = 0;
        let mut removed = 0;
        loop {
            let mut next = None;
            root.list(cursor, &mut |at, name, _| {
                next = Some((at, name.to_string()));
                false
            })
            .unwrap();
            let Some((at, name)) = next else { break };
            root.remove(&name).unwrap();
            removed += 1;
            cursor = at;
        }
        assert_eq!(removed, 5000);
        assert!(root.dir().unwrap().lock().is_empty());
    }

    /// A directory's piece has room for a few entries at first and grows as
    /// it fills, doubling up to a whole piece.
    #[test]
    fn a_small_directory_takes_little_room() {
        let fs = Tmpfs::new(1 << 20, u64::MAX, 0o755).unwrap();
        let root = &fs.root;
        let room =
            || root.dir().unwrap().lock().pieces.iter().map(KVec::capacity).collect::<Vec<_>>();
        root.create(&name(0), Kind::File, FILE).unwrap();
        assert_eq!(room(), [FIRST_ROOM]);
        let mut rooms = vec![FIRST_ROOM];
        for n in 1..=PIECE as u16 {
            root.create(&name(n), Kind::File, FILE).unwrap();
            let now = room();
            if now[0] != *rooms.last().unwrap() {
                rooms.push(now[0]);
            }
        }
        assert_eq!(rooms, [4, 8, 16, 32, 64, PIECE]);
        assert_eq!(room(), [PIECE, FIRST_ROOM]);
    }
}
