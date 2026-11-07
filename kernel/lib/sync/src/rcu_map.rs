// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A map from byte strings to values that readers look up without a lock.
//!
//! Lookups run in an RCU read section and take no lock: the map is a
//! table of hash chains whose links writers publish with one store and
//! free only after a grace period ([`rcu::defer`]), so a reader never
//! meets freed memory. Writers take the map's own mutex, one at a time.
//! A table that grows is built anew, with copies of its links, and
//! published whole: a reader walking the old one still finds every name
//! it held, since nothing in the old table changes once it is replaced.
//!
//! A lookup sees the map as one write left it: a name being added or
//! removed meanwhile is found or not, as if the lookup came before or
//! after.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::alloc::Allocator;
use core::mem::ManuallyDrop;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use crate::Mutex;
use crate::rcu::{self, Deferred};

/// Chains per table at first.
const FIRST_BUCKETS: usize = 8;

/// One name and its value, in a chain.
struct Link<V, A: Allocator> {
    next: AtomicPtr<Link<V, A>>,
    hash: u64,
    key: Vec<u8, A>,
    value: V,
    /// Its own allocator, to free it by from a deferred call.
    alloc: ManuallyDrop<A>,
    deferred: Deferred,
}

/// A table of chains, `mask + 1` of them.
struct Table<V, A: Allocator> {
    mask: usize,
    buckets: Vec<AtomicPtr<Link<V, A>>, A>,
    alloc: ManuallyDrop<A>,
    deferred: Deferred,
}

/// A name and value made ready to go in ([`RcuMap::pending`]), so that
/// putting it in cannot fail.
pub struct Pending<V, A: Allocator> {
    link: Box<Link<V, A>, A>,
}

/// The map. See the module's documentation.
pub struct RcuMap<V, A: Allocator + Clone> {
    table: AtomicPtr<Table<V, A>>,
    /// Names in the table.
    len: AtomicUsize,
    alloc: A,
    writer: Mutex<()>,
}

// SAFETY: readers on any CPU clone values (`V: Sync`), and values are
// dropped by whichever CPU makes the deferred call or drops the map
// (`V: Send`). The allocator moves into deferred calls too.
#[allow(unsafe_code)]
unsafe impl<V: Send + Sync, A: Allocator + Clone + Send + Sync> Sync for RcuMap<V, A> {}
// SAFETY: as above.
#[allow(unsafe_code)]
unsafe impl<V: Send + Sync, A: Allocator + Clone + Send + Sync> Send for RcuMap<V, A> {}

/// The hash of `key`: FxHash's, a word at a time, which is quick for the
/// short names of a directory. The bytes after the last whole word go in
/// as one more word ([`tail`]), not a step for each.
fn hash(key: &[u8]) -> u64 {
    const K: u64 = 0x517c_c1b7_2722_0a95;
    let mut hash = key.len() as u64;
    let (words, rest) = key.as_chunks::<8>();
    for word in words {
        hash = (hash.rotate_left(5) ^ u64::from_le_bytes(*word)).wrapping_mul(K);
    }
    if !rest.is_empty() {
        let (low, high) = tail(rest);
        hash = (hash.rotate_left(5) ^ (low | high << 32)).wrapping_mul(K);
    }
    hash
}

/// The bytes of `rest`, fewer than 8 and not none, in two loads that
/// between them cover each one, as two words: its first four bytes and its
/// last four, which overlap, for four to seven; for one to three, its
/// first, middle and last byte, which cover them. A loop over the bytes
/// costs a step for each; two loads cost the same whatever the length.
fn tail(rest: &[u8]) -> (u64, u64) {
    let n = rest.len();
    if let (Some(first), Some(last)) = (rest.first_chunk::<4>(), rest.last_chunk::<4>()) {
        (u64::from(u32::from_le_bytes(*first)), u64::from(u32::from_le_bytes(*last)))
    } else {
        let byte = |at: usize| rest.get(at).copied().map_or(0, u64::from);
        (byte(0) | byte(n / 2) << 8 | byte(n.wrapping_sub(1)) << 16, 0)
    }
}

/// Whether `a` and `b` hold the same bytes: a word at a time, which for a
/// short name costs less than the call `==` makes.
fn same(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let (a_words, a_rest) = a.as_chunks::<8>();
    let (b_words, b_rest) = b.as_chunks::<8>();
    a_words.iter().zip(b_words).all(|(x, y)| x == y)
        && (a_rest.is_empty() || tail(a_rest) == tail(b_rest))
}

/// Frees a link that left its chain, from its deferred call.
///
/// # Safety
///
/// `item` is the `deferred` of a `Link` made by `Box::into_raw_with_allocator`,
/// in no chain any reader can still reach.
#[allow(unsafe_code)]
unsafe fn free_link<V, A: Allocator>(item: NonNull<Deferred>) {
    let link = item.as_ptr().wrapping_byte_sub(core::mem::offset_of!(Link<V, A>, deferred));
    let link = link.cast::<Link<V, A>>();
    // SAFETY: the caller's guarantee: the link is ours to free, with the
    // allocator it keeps, which is read out once.
    unsafe {
        let alloc = ManuallyDrop::take(&mut (*link).alloc);
        drop(Box::from_raw_in(link, alloc));
    }
}

/// Frees a table that was replaced, and every link still in it, from its
/// deferred call.
///
/// # Safety
///
/// As [`free_link`], for a `Table` no reader can still reach, whose links
/// are in no other table.
#[allow(unsafe_code)]
unsafe fn free_table<V, A: Allocator>(item: NonNull<Deferred>) {
    let table = item.as_ptr().wrapping_byte_sub(core::mem::offset_of!(Table<V, A>, deferred));
    let table = table.cast::<Table<V, A>>();
    // SAFETY: the caller's guarantee.
    unsafe {
        let alloc = ManuallyDrop::take(&mut (*table).alloc);
        let table = Box::from_raw_in(table, alloc);
        drop_links(&table);
        drop(table);
    }
}

/// Frees every link in `table`'s chains.
///
/// # Safety
///
/// No reader may reach the table any more, and its links are in no other.
#[allow(unsafe_code)]
unsafe fn drop_links<V, A: Allocator>(table: &Table<V, A>) {
    for bucket in &table.buckets {
        let mut at = bucket.load(Ordering::Acquire);
        while let Some(link) = NonNull::new(at) {
            // SAFETY: a link in a chain is live and the table's; it is
            // read for its next before it goes.
            unsafe {
                at = link.as_ref().next.load(Ordering::Acquire);
                free_link::<V, A>(NonNull::from(&link.as_ref().deferred));
            }
        }
    }
}

impl<V: Clone + Send + Sync, A: Allocator + Clone + Send + Sync> RcuMap<V, A> {
    /// An empty map, whose memory comes from `alloc`.
    pub const fn new_in(alloc: A) -> Self {
        RcuMap {
            table: AtomicPtr::new(ptr::null_mut()),
            len: AtomicUsize::new(0),
            alloc,
            writer: Mutex::new(()),
        }
    }

    /// The value of `key`, if the map holds it: looked up without a lock.
    #[allow(unsafe_code)]
    pub fn get(&self, key: &[u8]) -> Option<V> {
        let hash = hash(key);
        let _section = rcu::read_lock();
        let table = NonNull::new(self.table.load(Ordering::Acquire))?;
        // SAFETY: a table and the links in its chains are freed only by a
        // deferred call made after they could no longer be reached, which
        // waits for this read section; until then they are not changed but
        // for `next`, which is atomic.
        unsafe {
            let table = table.as_ref();
            let mut at = table.buckets[(hash as usize) & table.mask].load(Ordering::Acquire);
            while let Some(link) = NonNull::new(at) {
                let link = link.as_ref();
                if link.hash == hash && same(&link.key, key) {
                    return Some(link.value.clone());
                }
                at = link.next.load(Ordering::Acquire);
            }
        }
        None
    }

    /// `key` and `value` made ready to go in with [`RcuMap::insert`]; none
    /// if there is no memory for them, or for the map's first table.
    pub fn pending(&self, key: &[u8], value: V) -> Option<Pending<V, A>> {
        if self.table.load(Ordering::Acquire).is_null() {
            let _writing = self.writer.lock();
            if self.table.load(Ordering::Relaxed).is_null() {
                self.table.store(self.table_of(FIRST_BUCKETS)?.as_ptr(), Ordering::Release);
            }
        }
        let copy = self.copy(key)?;
        let link = Link {
            next: AtomicPtr::new(ptr::null_mut()),
            hash: hash(key),
            key: copy,
            value,
            alloc: ManuallyDrop::new(self.alloc.clone()),
            deferred: Deferred::new(free_link::<V, A>),
        };
        Some(Pending { link: Box::try_new_in(link, self.alloc.clone()).ok()? })
    }

    /// Puts `pending` in. Its key must not be in the map. It cannot fail:
    /// a table that should grow and has no memory to stays as it is.
    // `pending` made the table, so the `expect` never fails.
    #[allow(unsafe_code, clippy::missing_panics_doc)]
    pub fn insert(&self, pending: Pending<V, A>) {
        let _writing = self.writer.lock();
        let len = self.len.load(Ordering::Relaxed) + 1;
        self.grow(len);
        // `pending` made the first table, and tables are only replaced.
        let table = NonNull::new(self.table.load(Ordering::Relaxed))
            .expect("a pending name's map has a table");
        let link = Box::into_raw_with_allocator(pending.link).0;
        // SAFETY: the table is the published one, which only writers
        // change, under the writer lock held here; the link is new.
        unsafe {
            let table = table.as_ref();
            let bucket = &table.buckets[((*link).hash as usize) & table.mask];
            (*link).next.store(bucket.load(Ordering::Relaxed), Ordering::Relaxed);
            bucket.store(link, Ordering::Release);
        }
        self.len.store(len, Ordering::Relaxed);
    }

    /// Takes `key` out, and frees it once no reader can see it; whether it
    /// was in.
    #[allow(unsafe_code)]
    pub fn remove(&self, key: &[u8]) -> bool {
        let _writing = self.writer.lock();
        let Some(table) = NonNull::new(self.table.load(Ordering::Relaxed)) else { return false };
        let hash = hash(key);
        // SAFETY: as in `insert`; a link taken out of its chain is in no
        // other, and goes to a deferred call that frees it.
        unsafe {
            let table = table.as_ref();
            let mut from = &table.buckets[(hash as usize) & table.mask];
            while let Some(link) = NonNull::new(from.load(Ordering::Relaxed)) {
                let found = link.as_ref();
                if found.hash == hash && found.key[..] == *key {
                    from.store(found.next.load(Ordering::Relaxed), Ordering::Release);
                    self.len.fetch_sub(1, Ordering::Relaxed);
                    rcu::defer(NonNull::from(&found.deferred));
                    return true;
                }
                from = &found.next;
            }
        }
        false
    }

    /// Makes a table of twice the chains once there are more names than
    /// chains (the first table at the first name), and publishes it; the
    /// old one goes with a deferred call. Without memory the table stays.
    /// Called with the writer lock held.
    #[allow(unsafe_code)]
    fn grow(&self, len: usize) {
        let old = NonNull::new(self.table.load(Ordering::Relaxed));
        // SAFETY: the published table, which only writers change.
        let buckets = old.map_or(0, |old| unsafe { old.as_ref() }.mask + 1);
        if len <= buckets {
            return;
        }
        let Some(new) = self.table_of((buckets * 2).max(FIRST_BUCKETS)) else { return };
        if let Some(old) = old {
            // SAFETY: as above; copies go in the new table, which no
            // reader sees yet, and a copy that fails frees what was made.
            unsafe {
                if !self.copy_links(old.as_ref(), new.as_ref()) {
                    free_table::<V, A>(NonNull::from(&new.as_ref().deferred));
                    return;
                }
            }
        }
        self.table.store(new.as_ptr(), Ordering::Release);
        if let Some(old) = old {
            // SAFETY: unpublished just now; its links are its own (the new
            // table holds copies).
            unsafe { rcu::defer(NonNull::from(&old.as_ref().deferred)) };
        }
    }

    /// A copy of `key`.
    fn copy(&self, key: &[u8]) -> Option<Vec<u8, A>> {
        let mut copy = Vec::new_in(self.alloc.clone());
        copy.try_reserve_exact(key.len()).ok()?;
        for &byte in key {
            // Room was reserved above.
            let _ = copy.push_within_capacity(byte);
        }
        Some(copy)
    }

    /// An empty table of `buckets` chains, a power of two.
    fn table_of(&self, buckets: usize) -> Option<NonNull<Table<V, A>>> {
        let mut chains = Vec::new_in(self.alloc.clone());
        chains.try_reserve_exact(buckets).ok()?;
        while chains.len() < buckets {
            // Room was reserved above.
            let _ = chains.push_within_capacity(AtomicPtr::new(ptr::null_mut()));
        }
        let table = Table {
            mask: buckets - 1,
            buckets: chains,
            alloc: ManuallyDrop::new(self.alloc.clone()),
            deferred: Deferred::new(free_table::<V, A>),
        };
        let table = Box::try_new_in(table, self.alloc.clone()).ok()?;
        NonNull::new(Box::into_raw_with_allocator(table).0)
    }

    /// Copies every link of `from` into `to`; false if memory ran out.
    #[allow(unsafe_code)]
    fn copy_links(&self, from: &Table<V, A>, to: &Table<V, A>) -> bool {
        for bucket in &from.buckets {
            let mut at = bucket.load(Ordering::Relaxed);
            while let Some(link) = NonNull::new(at) {
                // SAFETY: a link of the published table, live under the
                // writer lock.
                let link = unsafe { link.as_ref() };
                let Some(copy) = self.pending(&link.key, link.value.clone()) else { return false };
                let copy = Box::into_raw_with_allocator(copy.link).0;
                // SAFETY: the copy is new and the table unpublished.
                unsafe {
                    let chain = &to.buckets[((*copy).hash as usize) & to.mask];
                    (*copy).next.store(chain.load(Ordering::Relaxed), Ordering::Relaxed);
                    chain.store(copy, Ordering::Relaxed);
                }
                at = link.next.load(Ordering::Relaxed);
            }
        }
        true
    }
}

impl<V, A: Allocator + Clone> Drop for RcuMap<V, A> {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if let Some(table) = NonNull::new(*self.table.get_mut()) {
            // SAFETY: `&mut self`: nobody reads the map any more, and the
            // published table's links are its own.
            unsafe { free_table::<V, A>(NonNull::from(&table.as_ref().deferred)) };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::alloc::Global;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;

    /// `same` agrees with `==` for keys of every length up to a few
    /// words, differing in any one byte or not at all.
    #[test]
    fn keys_compare_a_word_at_a_time() {
        for len in 0..=20usize {
            let key: Vec<u8> = (0..len).map(|i| b'a' + i as u8).collect();
            assert!(same(&key, &key.clone()));
            assert_eq!(hash(&key), hash(&key.clone()));
            for at in 0..len {
                let mut other = key.clone();
                other[at] ^= 0x20;
                assert!(!same(&key, &other), "length {len}, byte {at}");
            }
            let mut longer = key.clone();
            longer.push(b'z');
            assert!(!same(&key, &longer));
        }
    }

    #[test]
    fn names_go_in_come_out_and_survive_growth() {
        crate::testing::install();
        let map = RcuMap::new_in(Global);
        assert_eq!(map.get(b"a"), None);
        for n in 0..1000u32 {
            let key = n.to_string();
            map.insert(map.pending(key.as_bytes(), n).unwrap());
        }
        for n in 0..1000u32 {
            assert_eq!(map.get(n.to_string().as_bytes()), Some(n));
        }
        assert_eq!(map.get(b"1000"), None);
        for n in (0..1000u32).step_by(2) {
            assert!(map.remove(n.to_string().as_bytes()));
        }
        assert!(!map.remove(b"0"));
        for n in 0..1000u32 {
            let found = map.get(n.to_string().as_bytes());
            assert_eq!(found, (n % 2 == 1).then_some(n));
        }
    }

    #[test]
    fn values_are_dropped_once() {
        crate::testing::install();
        let value = Arc::new(());
        {
            let map = RcuMap::new_in(Global);
            for n in 0..100u32 {
                map.insert(map.pending(n.to_string().as_bytes(), Arc::clone(&value)).unwrap());
            }
            for n in 0..50u32 {
                map.remove(n.to_string().as_bytes());
            }
            assert_eq!(Arc::strong_count(&value), 51);
        }
        assert_eq!(Arc::strong_count(&value), 1);
    }

    #[test]
    fn readers_find_every_name_that_stays_while_others_come_and_go() {
        crate::testing::install();
        let map = Arc::new(RcuMap::new_in(Global));
        for n in 0..64u32 {
            map.insert(map.pending(format!("stays{n}").as_bytes(), n).unwrap());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let readers: Vec<_> = (0..3)
            .map(|_| {
                let (map, stop) = (Arc::clone(&map), Arc::clone(&stop));
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        for n in 0..64u32 {
                            let key = format!("stays{n}");
                            assert_eq!(map.get(key.as_bytes()), Some(n), "lost {key}");
                        }
                    }
                })
            })
            .collect();
        for round in 0..20u32 {
            for n in 0..200u32 {
                map.insert(map.pending(format!("goes{round}-{n}").as_bytes(), n).unwrap());
            }
            for n in 0..200u32 {
                assert!(map.remove(format!("goes{round}-{n}").as_bytes()));
            }
        }
        stop.store(true, Ordering::Relaxed);
        for reader in readers {
            reader.join().unwrap();
        }
    }
}
