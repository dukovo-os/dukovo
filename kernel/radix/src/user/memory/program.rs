// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The pages of programs' files that every space maps alike (ADR 0563).
//!
//! A program's code and read-only data are the same bytes in every
//! process that runs it, yet each space filled a page of its own from the
//! file the first time it touched one: twenty services that run one
//! program held twenty copies of the code each of them ran. A file that
//! cannot change, one in a read-only Btrfs subvolume (as the system's
//! slots are) or in the startup archive, now has a [`ProgramPages`] while
//! a space maps it, which keeps the pages filled from it: a fault on a
//! whole page of the file, in a region the space may not write, maps the
//! page kept for it if there is one, and keeps the page it makes if not.
//! Whether a file can change is asked again at each such fault: a file
//! found to be able to change has no page kept for any space from then
//! on, though it could not change again later, and each space fills its
//! own. The `share-program-pages` flag, for measuring, takes every file
//! as one that cannot change (dukovo-boot(7)).
//!
//! A kept page is anonymous memory as any other (`Owner::Anon`). Its share
//! count counts the spaces that map it and one more for the
//! [`ProgramPages`], so it is never written in place: a space that
//! `mprotect`s it writable copies it at its first write, as after a fork,
//! and the kernel's writes to a new program (its relocations) copy it
//! first (`SpaceObject::touch`). Being shared, it is not swapped out. Each
//! space is charged for it while it maps it, as for a page of its own.
//!
//! A kept page that no space maps any more is freed when a region of its
//! file goes ([`ProgramPages::trim`]): a program's end frees the pages only
//! it used, and the end of the last region that maps the file frees the
//! rest with the [`ProgramPages`]. A page whose memory went bad (ADR 0168)
//! is handed to no space more.
//!
//! Lock order: an address space's regions, then a file's kept pages (then
//! the frame allocator's). The list of files is taken alone.

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use radix_mm::{FRAMES, Flags, HEAP, Kmalloc, Owner};
use radix_sync::Mutex;
use radix_vfs::NodeRef;

use super::anon::{CHUNK, Chunk, unshare};

/// The pages kept of one file that programs map, while a space maps it.
pub struct ProgramPages {
    /// The file, which its pages keep alive and so at its address.
    #[expect(dead_code, reason = "held only to keep the file at its address")]
    node: NodeRef,
    /// The frames of its kept pages, in chunks of [`CHUNK`] pages kept in
    /// order of their number; 0 for a page not kept. Allocated without
    /// sleeping, with the chunks locked: where memory is that short, a
    /// page is made for its space alone.
    chunks: Mutex<Vec<Chunk, Kmalloc>>,
    /// Whether the file was found to be able to change after its pages
    /// were kept: they are handed to no space more.
    stale: AtomicBool,
}

/// A file's kept pages, shared by the regions that map the file.
pub type ProgramRef = Arc<ProgramPages, Kmalloc>;

/// A file that has pages kept: the address of its node, and its pages
/// while a region maps it.
type File = (usize, Weak<ProgramPages, Kmalloc>);

/// The files that have pages kept, in order of the address of their node:
/// those no region maps any more are gone, and their entries are left out
/// when the next is added.
static FILES: Mutex<Option<Vec<File, Kmalloc>>> = Mutex::new(None);

/// The address of a file's node, which tells it from every other file
/// while it lives.
fn key_of(node: &NodeRef) -> usize {
    Arc::as_ptr(node).cast::<()>().addr()
}

/// Whether the pages of `node` may be kept for every space: it cannot
/// change, or `share-program-pages` takes every file so, for measuring.
fn cannot_change(node: &NodeRef) -> bool {
    node.read_only() || crate::settings().share_program_pages
}

/// How the chunks are allocated: without sleeping, as they are with a
/// file's pages locked.
fn atomic() -> Kmalloc {
    HEAP.with(Flags::ATOMIC)
}

impl ProgramPages {
    /// The pages kept of the file `node` for the spaces that map it, made
    /// now if none maps it yet: `None` for a file that can change, whose
    /// pages each space fills for itself, or with memory short.
    pub fn of(node: &NodeRef) -> Option<ProgramRef> {
        if !cannot_change(node) {
            return None;
        }
        let key = key_of(node);
        let mut files = FILES.lock();
        let files = files.get_or_insert_with(|| Vec::new_in(super::kmalloc()));
        let found = files.binary_search_by_key(&key, |&(at, _)| at);
        if let Ok(at) = found
            && let Some(pages) = files[at].1.upgrade()
        {
            return Some(pages);
        }
        let pages = ProgramPages {
            node: node.clone(),
            chunks: Mutex::new(Vec::new_in(atomic())),
            stale: AtomicBool::new(false),
        };
        let pages = Arc::try_new_in(pages, super::kmalloc()).ok()?;
        if let Ok(at) = found {
            // The entry of a file whose pages went; this is the same node,
            // or a new one at its address.
            files[at].1 = Arc::downgrade(&pages);
            return Some(pages);
        }
        files.retain(|(_, weak)| weak.strong_count() > 0);
        files.try_reserve(1).ok()?;
        let at = files.partition_point(|&(at, _)| at < key);
        // Room was reserved; the list stays in order.
        let _ = files.push_within_capacity((key, Arc::downgrade(&pages)));
        files[at..].rotate_right(1);
        Some(pages)
    }

    /// Whether its pages may be handed out: the file, `node`, still
    /// cannot change, nor could at any fault since they were first kept.
    pub(super) fn usable(&self, node: &NodeRef) -> bool {
        if self.stale.load(Ordering::Acquire) {
            return false;
        }
        if !cannot_change(node) {
            self.stale.store(true, Ordering::Release);
            return false;
        }
        true
    }

    /// The frame kept for page `index` of the file, counted as shared once
    /// more for the space about to map it: `None` if none is kept. A kept
    /// page whose memory went bad is let go of here, and a new one is made
    /// in its place; the spaces that map it keep it until they let go.
    pub(super) fn take(&self, index: u64) -> Option<u64> {
        let mut chunks = self.chunks.lock();
        let at = chunks.binary_search_by_key(&(index / CHUNK), |&(number, _)| number).ok()?;
        let slot = &mut chunks[at].1[(index % CHUNK) as usize];
        let phys = *slot;
        if phys == 0 {
            return None;
        }
        if FRAMES.retiring(phys) {
            *slot = 0;
            unshare(phys);
            return None;
        }
        FRAMES.info(phys)?.share();
        Some(phys)
    }

    /// Keeps `phys`, a page just made and filled from page `index` of the
    /// file for a space about to map it, for the spaces that map that page
    /// after it. Returns the frame the space maps: `phys`, or the page
    /// another space kept meanwhile, counted as shared once more, in which
    /// case `phys` is freed. A page there is no room to keep stays the
    /// space's own.
    pub(super) fn keep(&self, index: u64, phys: u64) -> u64 {
        let mut chunks = self.chunks.lock();
        let number = index / CHUNK;
        let at = match chunks.binary_search_by_key(&number, |&(number, _)| number) {
            Ok(at) => at,
            Err(at) => {
                let mut chunk = Vec::new_in(atomic());
                if chunks.try_reserve(1).is_err()
                    || chunk.try_reserve_exact(CHUNK as usize).is_err()
                {
                    return phys;
                }
                // The room for both was reserved; the chunks stay in order.
                for _ in 0..CHUNK {
                    let _ = chunk.push_within_capacity(0);
                }
                let _ = chunks.push_within_capacity((number, chunk));
                chunks[at..].rotate_right(1);
                at
            }
        };
        let slot = &mut chunks[at].1[(index % CHUNK) as usize];
        let kept = *slot;
        if kept != 0 && !FRAMES.retiring(kept) {
            // Another space made the page meanwhile: this one maps it too.
            if let Some(info) = FRAMES.info(kept) {
                info.share();
            }
            drop(chunks);
            let _ = FRAMES.free(phys, 0, Owner::Anon);
            return kept;
        }
        if kept != 0 {
            unshare(kept);
        }
        *slot = phys;
        if let Some(info) = FRAMES.info(phys) {
            info.share();
        }
        phys
    }

    /// Frees the pages kept from page `first` of the file to `end` that no
    /// space maps any more: a region that mapped them went.
    pub(super) fn trim(&self, first: u64, end: u64) {
        let mut chunks = self.chunks.lock();
        let mut emptied = false;
        let from = chunks.partition_point(|&(number, _)| (number + 1) * CHUNK <= first);
        for (number, chunk) in &mut chunks[from..] {
            let base = *number * CHUNK;
            if base >= end {
                break;
            }
            let start = first.saturating_sub(base) as usize;
            let stop = (end - base).min(CHUNK) as usize;
            for slot in &mut chunk[start..stop] {
                // Only these pages hold it, and nothing makes it shared
                // again but `take`, which waits for them.
                if *slot != 0 && FRAMES.info(*slot).is_some_and(|info| info.share_count() == 1) {
                    unshare(*slot);
                    *slot = 0;
                    emptied = true;
                }
            }
        }
        if emptied {
            chunks.retain(|(_, chunk)| chunk.iter().any(|&phys| phys != 0));
        }
    }
}

impl Drop for ProgramPages {
    /// Lets go of every page it kept: no region maps the file any more,
    /// and so no space any of its pages.
    fn drop(&mut self) {
        for (_, chunk) in self.chunks.get_mut().iter() {
            for &phys in chunk.iter().filter(|&&phys| phys != 0) {
                unshare(phys);
            }
        }
    }
}
