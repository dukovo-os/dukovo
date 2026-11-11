// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Two trees compared (ADR 0462): the items one has that the other has
//! not, or has with other data, in key order, as Linux's
//! `btrfs_compare_trees` finds them for `btrfs send`.
//!
//! A snapshot shares every block with the subvolume it was taken of until
//! one of the two changes it, and a changed block is a copy at a new
//! address. So two snapshots of one subvolume share the blocks of all
//! that did not change between them, and the walk goes down both trees
//! together, level by level, past every block the two share: a block
//! names the same subtree in both when the pointers to it hold the same
//! address and transaction. Only the leaves of what changed are read, and
//! their items compared one by one.
//!
//! The walk starts at any key, down to that key's leaf in each tree, so a
//! comparison too long for one call goes on in the next from the key
//! after the last item it gave.

use core::cmp::Ordering;

use radix_vfs::mem::KArc;
use radix_vfs::{Error, Result};

use crate::format::Key;
use crate::tree::{Block, Bounds, MAX_LEVEL, Root, read_block};
use crate::volume::Volume;

/// One tree's place in the walk: the block and slot at each level from
/// its root down to the level the walk stands at, which may be a node's
/// key pointer as well as a leaf's item.
struct Side<'v> {
    volume: &'v Volume,
    path: [Option<(KArc<Block>, usize)>; MAX_LEVEL as usize],
    /// The root's level.
    top: usize,
    /// The level the walk stands at: 0 at an item.
    level: usize,
    /// Whether the walk has passed the tree's last item.
    end: bool,
}

/// What to do with a side next.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Nothing.
    Stay,
    /// On to the next item or key pointer at its level, up a level past
    /// the end of a block: past a whole subtree the other side shares.
    Next,
    /// Down into the subtree of the key pointer it stands at, to its
    /// first key pointer or item.
    Down,
}

impl<'v> Side<'v> {
    /// A side past its end: the tree that is not there, for a comparison
    /// with nothing.
    fn none(volume: &'v Volume) -> Side<'v> {
        Side { volume, path: Default::default(), top: 0, level: 0, end: true }
    }

    /// The tree `root` at its first item whose key is `from` or after it.
    #[inline(never)]
    fn at(volume: &'v Volume, root: &Root, from: &Key) -> Result<Side<'v>> {
        let top = usize::from(root.level);
        let mut side = Side { volume, path: Default::default(), top, level: 0, end: false };
        let mut block = read_block(volume, root, &Bounds::default())?;
        loop {
            let level = usize::from(block.level());
            let slot = block.lower_bound(from);
            if block.is_leaf() {
                side.path[0] = Some((block, slot));
                break;
            }
            // Down the last child whose first key is `from` or before it.
            let slot = if slot < block.nritems() && block.key(slot) == *from {
                slot
            } else {
                slot.saturating_sub(1)
            };
            let child = block.child(slot);
            let bounds = Bounds::of_child(&block, slot);
            side.path[level] = Some((block, slot));
            block = read_block(volume, &child, &bounds)?;
        }
        // A leaf whose keys are all before `from` goes on at the next; an
        // empty one is an empty tree's root.
        let (leaf, slot) = side.path[0].as_mut().ok_or(Error::Corrupt)?;
        if *slot >= leaf.nritems() {
            if leaf.nritems() == 0 {
                side.end = true;
                return if top == 0 { Ok(side) } else { Err(Error::Corrupt) };
            }
            *slot -= 1;
            side.next()?;
        }
        Ok(side)
    }

    /// The block and slot the walk stands at.
    fn here(&self) -> Result<&(KArc<Block>, usize)> {
        self.path[self.level].as_ref().ok_or(Error::Corrupt)
    }

    /// The key it stands at: an item's, or the first key of a key
    /// pointer's subtree.
    fn key(&self) -> Result<Key> {
        let (block, slot) = self.here()?;
        Ok(block.key(*slot))
    }

    /// On to the next item or key pointer at its level, up past the end
    /// of each block, as Linux's `tree_next_item` goes.
    fn next(&mut self) -> Result<()> {
        loop {
            let (block, slot) = self.path[self.level].as_mut().ok_or(Error::Corrupt)?;
            *slot += 1;
            if *slot < block.nritems() {
                return Ok(());
            }
            if self.level >= self.top {
                self.end = true;
                return Ok(());
            }
            self.path[self.level] = None;
            self.level += 1;
        }
    }

    /// Down into the subtree of the key pointer it stands at.
    fn down(&mut self) -> Result<()> {
        if self.level == 0 {
            return Err(Error::Corrupt);
        }
        let (node, slot) = self.here()?;
        let child = node.child(*slot);
        let bounds = Bounds::of_child(node, *slot);
        let block = read_block(self.volume, &child, &bounds)?;
        if block.nritems() == 0 {
            return Err(Error::Corrupt);
        }
        self.level -= 1;
        self.path[self.level] = Some((block, 0));
        Ok(())
    }

    fn take(&mut self, step: Step) -> Result<()> {
        match step {
            Step::Stay => Ok(()),
            Step::Next => self.next(),
            Step::Down => self.down(),
        }
    }

    /// The item it stands at, at a leaf.
    fn item(&self) -> Result<&[u8]> {
        let (block, slot) = self.here()?;
        Ok(block.item(*slot))
    }

    /// The subtree the key pointer it stands at names: its address and
    /// the transaction that wrote it.
    fn subtree(&self) -> Result<(u64, u64)> {
        let (block, slot) = self.here()?;
        let child = block.child(*slot);
        Ok((child.bytenr, child.generation))
    }
}

/// Compares the items of tree `new` with those of tree `old`, or with
/// none if there is no `old`, for the keys from `from` to `to`, both
/// included: gives `each` the key of every item one of them has that the
/// other has not, or has with other data, in key order, with its data in
/// `old` and in `new` (`None` where it is not). `each` returns false to
/// stop before that item, which is then not taken.
///
/// Returns true once every key to `to` is compared, and false if `each`
/// stopped it: the caller goes on from the key after the last item taken.
/// The trees are of one filesystem and do not change while they are read:
/// read-only snapshots, under the transaction on a writable mount.
#[inline(never)]
pub fn compare(
    volume: &Volume,
    old: Option<&Root>,
    new: &Root,
    from: &Key,
    to: &Key,
    mut each: impl FnMut(&Key, Option<&[u8]>, Option<&[u8]>) -> bool,
) -> Result<bool> {
    let mut left = match old {
        Some(root) => Side::at(volume, root, from)?,
        None => Side::none(volume),
    };
    let mut right = Side::at(volume, new, from)?;
    let (mut go_left, mut go_right) = (Step::Stay, Step::Stay);
    loop {
        left.take(go_left)?;
        right.take(go_right)?;
        (go_left, go_right) = (Step::Stay, Step::Stay);
        // Everything after `to` is left for another call: the keys a walk
        // stands at only grow, and a key pointer's is its subtree's first.
        let past = |side: &Side<'_>| -> Result<bool> { Ok(side.end || side.key()? > *to) };
        if past(&left)? && past(&right)? {
            return Ok(true);
        }
        if left.end || right.end {
            // What is left of one tree is all one side's.
            let (side, go, gone) =
                if left.end { (&right, &mut go_right, false) } else { (&left, &mut go_left, true) };
            if side.level > 0 {
                *go = Step::Down;
                continue;
            }
            let (key, item) = (side.key()?, side.item()?);
            let (old, new) = if gone { (Some(item), None) } else { (None, Some(item)) };
            if !each(&key, old, new) {
                return Ok(false);
            }
            *go = Step::Next;
            continue;
        }
        let (l, r) = (left.key()?, right.key()?);
        if left.level == 0 && right.level == 0 {
            // Two items: the one whose key is first is the other's to
            // lack, and one key in both may have other data.
            match l.cmp(&r) {
                Ordering::Less => {
                    if l <= *to && !each(&l, Some(left.item()?), None) {
                        return Ok(false);
                    }
                    go_left = Step::Next;
                }
                Ordering::Greater => {
                    if r <= *to && !each(&r, None, Some(right.item()?)) {
                        return Ok(false);
                    }
                    go_right = Step::Next;
                }
                Ordering::Equal => {
                    let (a, b) = (left.item()?, right.item()?);
                    if a != b && l <= *to && !each(&l, Some(a), Some(b)) {
                        return Ok(false);
                    }
                    (go_left, go_right) = (Step::Next, Step::Next);
                }
            }
        } else if left.level == right.level {
            // Two key pointers: one subtree named by both is shared, and
            // both go past it; otherwise down the one whose key is first,
            // or both.
            match l.cmp(&r) {
                Ordering::Less => go_left = Step::Down,
                Ordering::Greater => go_right = Step::Down,
                Ordering::Equal if left.subtree()? == right.subtree()? => {
                    (go_left, go_right) = (Step::Next, Step::Next);
                }
                Ordering::Equal => (go_left, go_right) = (Step::Down, Step::Down),
            }
        } else if left.level < right.level {
            go_right = Step::Down;
        } else {
            go_left = Step::Down;
        }
    }
}
