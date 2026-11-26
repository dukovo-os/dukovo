// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The names a directory has looked up, each with the node it led to,
//! for the lookups after (ADR 0659).
//!
//! A lookup on a writable mount takes the filesystem's transaction, walks
//! the directory's tree to the name's entry and takes the table of nodes
//! in use: three locks every CPU shares. Programs that open files by
//! their full paths look up the same directories over and over, and each
//! waited for the others' lookups. A directory keeps what each name it
//! found led to, with the count of changes to its entries as the lookup
//! began ([`Volume::names_seen`]), and a lookup that finds the name kept
//! while the count says the same takes no lock, as tmpfs's do (ADR 0367).
//!
//! The count moves after every change to the directory's entries
//! (`Writer`'s edits of its `DIR_ITEM`s), so a name kept while it says the
//! same leads where a walk of the tree would: the lookup that kept it read
//! the count before its walk, and a change its walk missed moved the count
//! past what it read. Directories share the volume's few counts, so a
//! change may send another directory's lookups to the tree once; a
//! subvolume removed moves them all. A name keeps its node weakly, as
//! tmpfs's do: a node nobody holds goes, and its name is looked up again.
//!
//! [`Volume::names_seen`]: crate::volume::Volume::names_seen

use alloc::sync::Weak;

use radix_sync::rcu_map::RcuMap;
use radix_vfs::mem::{self, Alloc, KArc};

use crate::node::BtrfsNode;

/// The most names a directory keeps: one more empties it, and it starts
/// again.
const NAMES_MAX: usize = 64;

/// What a name led to, and the count of changes when its lookup began.
#[derive(Clone)]
struct Named {
    seen: u64,
    node: Weak<BtrfsNode, Alloc>,
}

/// A directory's names, read without a lock.
pub(crate) struct Names(RcuMap<Named, Alloc>);

impl Names {
    /// None yet, and no memory until the first.
    pub(crate) fn new() -> Names {
        Names(RcuMap::new_in(mem::alloc()))
    }

    /// The node `name` led to, if a lookup that began while the count of
    /// changes was `seen` kept it and it is still in use.
    pub(crate) fn find(&self, name: &str, seen: u64) -> Option<KArc<BtrfsNode>> {
        let kept = |named: &Named| if named.seen == seen { named.node.upgrade() } else { None };
        self.0.get_with(name.as_bytes(), kept).flatten()
    }

    /// Keeps `node`, where `name` led a lookup that began with the count
    /// of changes at `seen`, in place of what the name led to before. With
    /// no memory for it, the name is not kept.
    pub(crate) fn keep(&self, name: &str, seen: u64, node: &KArc<BtrfsNode>) {
        if self.0.len() >= NAMES_MAX {
            self.0.clear();
        }
        let named = Named { seen, node: KArc::downgrade(node) };
        if let Some(pending) = self.0.pending(name.as_bytes(), named) {
            self.0.put(pending);
        }
    }
}
