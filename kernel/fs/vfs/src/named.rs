// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A directory with only some names in it (ADR 0473): what a view
//! attaches when a service may reach some entries of a directory and
//! not the rest, as the updater reaches the system's subvolumes at the
//! top of the root filesystem but not `@users` beside them.
//!
//! [`Named`] stands for the directory in the view. Every method that
//! takes a name lets through only the names it was given: a lookup or a
//! removal of another is not found, making one is not permitted, and a
//! listing leaves the others out, so nothing past the names is reached
//! through it, by a walk, a capability opened on it, or `..` from beneath
//! it, which the walk takes back along its chain to this node. Beneath a
//! name, the directory's own nodes are reached unchanged.
//!
//! Every method of [`Node`] is written out here, none left to the
//! trait's default, so that a method added to the trait later does not
//! pass through unchecked: until it is written here, the kernel does not
//! build.
//!
//! The directory's filesystem finds its own node type behind a second
//! node, as `rename`'s target and `snapshot`'s source: the VFS hands it
//! the directory itself in place of this node ([`unwrap_dir`]), after
//! checking the name, and refuses this node as a snapshot's source
//! ([`is_named`]), which would be the whole top level.

use core::any::Any;

use crate::Access;
use crate::mem::{self, KVec};
use crate::{
    AccessChange, Error, In, KArc, Kind, ListFn, Node, NodeRef, Out, Result, Setting,
    SocketBinding, Stat, Subvolume, TreeItem, TreeRange, acl, path,
};

/// The most names a directory may be attached with.
pub const NAMES_MAX: usize = 16;

/// A directory with only `names` in it.
pub struct Named {
    inner: NodeRef,
    names: KVec<KVec<u8>>,
}

impl Named {
    /// `dir`, a directory, with only `names` in it: each a plain name
    /// ([`path::check_name`]), at most [`NAMES_MAX`] of them.
    pub fn make(dir: NodeRef, names: &[&str]) -> Result<NodeRef> {
        if dir.kind() != Kind::Directory {
            return Err(Error::NotDirectory);
        }
        if names.is_empty() || names.len() > NAMES_MAX {
            return Err(Error::InvalidArgument);
        }
        let mut kept = mem::vec_with_capacity(names.len())?;
        for name in names {
            path::check_name(name)?;
            let _ = kept.push_within_capacity(mem::copy(name.as_bytes())?);
        }
        let node: NodeRef = mem::arc(Named { inner: dir, names: kept })?;
        Ok(node)
    }

    /// Whether `name` is one of its names.
    fn has(&self, name: &str) -> bool {
        self.names.iter().any(|kept| kept[..] == *name.as_bytes())
    }

    /// `Ok` if `name` is one of its names, otherwise `refused`.
    fn check(&self, name: &str, refused: Error) -> Result<()> {
        if self.has(name) { Ok(()) } else { Err(refused) }
    }
}

/// Whether `node` is a [`Named`].
pub fn is_named(node: &dyn Node) -> bool {
    node.as_any().is::<Named>()
}

/// `dir`, a directory something is made in or moved to under `name`:
/// the directory itself if `dir` is a [`Named`] and `name` one of its
/// names, for its filesystem to find its own node; refused if `name` is
/// not one of them; `dir` unchanged otherwise.
pub fn unwrap_dir<'a>(dir: &'a dyn Node, name: &str) -> Result<&'a dyn Node> {
    match dir.as_any().downcast_ref::<Named>() {
        Some(named) => {
            named.check(name, Error::NotPermitted)?;
            Ok(&*named.inner)
        }
        None => Ok(dir),
    }
}

impl Node for Named {
    fn stat(&self) -> Stat {
        self.inner.stat()
    }

    fn kind(&self) -> Kind {
        Kind::Directory
    }

    fn read_only(&self) -> bool {
        self.inner.read_only()
    }

    fn open(&self) -> Result<Option<NodeRef>> {
        // A directory's handle keeps the node it was opened through: this
        // one, so that what is opened beneath it is checked too.
        Ok(None)
    }

    fn opening(&self, want: u16) -> Result<()> {
        self.inner.opening(want)
    }

    fn lookup(&self, name: &str) -> Result<NodeRef> {
        self.check(name, Error::NotFound)?;
        self.inner.lookup(name)
    }

    fn access(&self) -> Access {
        self.inner.access()
    }

    fn set_access(&self, change: &AccessChange) -> Result<()> {
        self.inner.set_access(change)
    }

    fn create(&self, name: &str, kind: Kind, access: Access) -> Result<NodeRef> {
        self.check(name, Error::NotPermitted)?;
        self.inner.create(name, kind, access)
    }

    fn make_link(&self, name: &str, target: &str, access: Access) -> Result<NodeRef> {
        self.check(name, Error::NotPermitted)?;
        self.inner.make_link(name, target, access)
    }

    fn link(&self, name: &str, node: &NodeRef) -> Result<()> {
        self.check(name, Error::NotPermitted)?;
        if is_named(&**node) {
            return Err(Error::IsDirectory);
        }
        self.inner.link(name, node)
    }

    fn make_node(&self, name: &str, kind: Kind, device: u64, access: Access) -> Result<NodeRef> {
        self.check(name, Error::NotPermitted)?;
        self.inner.make_node(name, kind, device, access)
    }

    fn make_socket(&self, name: &str, binding: SocketBinding, access: Access) -> Result<NodeRef> {
        self.check(name, Error::NotPermitted)?;
        self.inner.make_socket(name, binding, access)
    }

    fn socket_binding(&self) -> Option<SocketBinding> {
        None
    }

    fn read_link(&self, _out: &mut dyn Out) -> Result<usize> {
        Err(Error::NotLink)
    }

    fn remove(&self, name: &str) -> Result<()> {
        self.check(name, Error::NotFound)?;
        self.inner.remove(name)
    }

    fn make_subvolume(&self, name: &str) -> Result<()> {
        self.check(name, Error::NotPermitted)?;
        self.inner.make_subvolume(name)
    }

    fn snapshot(&self, name: &str, source: &NodeRef, read_only: bool) -> Result<()> {
        self.check(name, Error::NotPermitted)?;
        // The whole top level, past the names: never.
        if is_named(&**source) {
            return Err(Error::NotPermitted);
        }
        self.inner.snapshot(name, source, read_only)
    }

    fn remove_subvolume(&self, name: &str) -> Result<()> {
        self.check(name, Error::NotFound)?;
        self.inner.remove_subvolume(name)
    }

    fn subvolume(&self) -> Result<Subvolume> {
        self.inner.subvolume()
    }

    fn compare(
        &self,
        _parent: Option<&NodeRef>,
        _range: &TreeRange,
        _each: &mut dyn FnMut(&TreeItem<'_>) -> bool,
    ) -> Result<bool> {
        // The tree it is the top of holds the names left out.
        Err(Error::NotPermitted)
    }

    fn mark_received(&self, _uuid: &[u8; 16], _sent_transaction: u64, _sent: u64) -> Result<()> {
        Err(Error::NotPermitted)
    }

    fn rename(&self, from: &str, to_dir: &dyn Node, to: &str) -> Result<()> {
        self.check(from, Error::NotFound)?;
        // Moved within itself, the target is this node: the directory
        // itself once `to` is checked.
        self.inner.rename(from, unwrap_dir(to_dir, to)?, to)
    }

    fn list(&self, cursor: u64, f: ListFn<'_>) -> Result<()> {
        self.inner.list(cursor, &mut |at, name, kind| !self.has(name) || f(at, name, kind))
    }

    fn read(&self, _offset: u64, _out: &mut dyn Out) -> Result<usize> {
        Err(Error::IsDirectory)
    }

    fn write(&self, _offset: u64, _data: &dyn In) -> Result<usize> {
        Err(Error::IsDirectory)
    }

    fn set_size(&self, _size: u64) -> Result<()> {
        Err(Error::IsDirectory)
    }

    fn clone_range(&self, _source: &NodeRef, _from: u64, _len: u64, _to: u64) -> Result<()> {
        Err(Error::IsDirectory)
    }

    fn set_times(&self, accessed: Option<u64>, modified: Option<u64>) -> Result<()> {
        self.inner.set_times(accessed, modified)
    }

    fn get_attribute(&self, name: &[u8]) -> Result<KVec<u8>> {
        self.inner.get_attribute(name)
    }

    fn set_attribute(&self, name: &[u8], value: &[u8], how: Setting) -> Result<()> {
        self.inner.set_attribute(name, value, how)
    }

    fn list_attributes(&self) -> Result<KVec<u8>> {
        self.inner.list_attributes()
    }

    fn remove_attribute(&self, name: &[u8]) -> Result<()> {
        self.inner.remove_attribute(name)
    }

    fn access_list(&self) -> Result<Option<KArc<acl::Acl>>> {
        self.inner.access_list()
    }

    fn default_list(&self) -> Result<Option<KArc<acl::Acl>>> {
        self.inner.default_list()
    }

    fn sync(&self) -> Result<()> {
        self.inner.sync()
    }

    fn block_device(&self) -> Option<radix_block::DeviceRef> {
        None
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
