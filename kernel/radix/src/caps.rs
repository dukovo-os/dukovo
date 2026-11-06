// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel's capability store.
//!
//! One [`Capabilities`] store (`radix-cap`) holds every capability space.
//! Its memory comes from the kernel heap. Memory objects take their pages
//! from the frame allocator, owned by [`Owner::Memory`], and give them
//! back when the store releases them.
//!
//! The store has its own locks (ADR 0369): one per capability space, a
//! tree lock over the derivation tree's links and one over its object
//! table. Work on one space's capabilities that follows no link of the
//! tree (looking one up, making an object, deleting or moving a
//! capability nothing was derived from) takes that space's lock alone
//! ([`in_space`]), so programs in different spaces never wait for each
//! other. Work that follows or changes a link (deriving, a delete or a
//! move of a capability with relatives, a revoke) takes the tree lock and
//! the spaces it names ([`with_tree`]). The locks are mutexes, so a
//! thread waiting for one sleeps, and the store may allocate with
//! `MaySleep` while holding them (the out-of-memory policy may then sleep
//! too; it never needs the store). Revokes run in steps of
//! [`REVOKE_STEP`] deletions and let go of the locks between steps,
//! yielding the CPU, so other threads are not shut out for long.
//!
//! The registry of the kernel objects the capabilities name
//! (`user::objects`) lives beside the store's object table, under the
//! table's lock: an object is recorded in the hold that names it and
//! leaves in the hold that releases it (ADR 0108). Lock order: the tree
//! lock, the spaces' locks by increasing index, the object table's; then
//! whatever an object takes when it closes ([`release`]).
//!
//! The store has a version number for the caches outside it: each CPU
//! keeps the capabilities its system calls found, with the version the
//! store had before the lookup (`user::objects`), and the version moves
//! whenever a capability some CPU found leaves its slot, or an object
//! some CPU found leaves the registry ([`mark_stale`]), before the lock
//! that guarded it goes. Other changes, such as a capability put in an
//! empty slot or one nobody looked up deleted, leave every cache as it
//! is. So programs on different CPUs using their capabilities at once do
//! not contend, and opening and closing files does not empty the caches.
//!
//! What the store releases is closed under its locks, but some of the
//! work that follows waits until they have gone ([`finish`]): processes
//! with no thread left are finished, removed files dropped, cards
//! stopped and parents told.
//!
//! Resource domains (`domains`) are objects in the store too: a domain
//! lives while a capability to it does, and goes when the last one is
//! deleted or revoked.

use core::fmt;

use radix_cap::{
    CapError, Capabilities, Capability, Derivation, MAX_DEPTH, MAX_SLOTS, ObjectId, ObjectKind,
    Progress, Released, Rights, SlotRef, SpaceId, SpaceView, TreeView,
};
use radix_klog::{error, info};
use radix_mm::{FRAMES, Flags, HEAP, Kmalloc, Owner, PAGE_SHIFT};
use radix_sync::Once;

use crate::domains::{self, DomainError, DomainRef, Settings};
use crate::user::objects::{self, Registry};

/// The store's type in the kernel, with the registry beside its object
/// table.
pub type Store = Capabilities<Kmalloc, Registry>;

/// One capability space, locked: the fast path's view ([`in_space`]).
pub type Space<'v> = SpaceView<'v, Kmalloc, Registry>;

/// The derivation tree and the spaces named, locked: the slow path's
/// view ([`with_tree`]).
pub type Tree<'v> = TreeView<'v, Kmalloc, Registry>;

/// The store, once [`init`] has run.
static STORE: Once<Store> = Once::new();

/// Capabilities a revoke deletes before it lets go of the locks.
pub const REVOKE_STEP: usize = 64;

/// Sets up the store and reports it.
pub fn init() {
    let store = match Capabilities::new(HEAP.with(Flags::MAY_SLEEP), Registry::new()) {
        Ok(store) => store,
        Err(err) => panic!("no memory for the capability store at boot: {err}"),
    };
    let _ = STORE.set(store);
    info!(
        "Capabilities: ready; {} object kinds, {} rights, spaces of up to {MAX_SLOTS} slots, \
         derivations up to {MAX_DEPTH} deep",
        ObjectKind::ALL.len(),
        Rights::ALL.len()
    );
}

/// The store, if it is set up. Each of its methods takes the locks it
/// needs and lets them go again; one that may release objects takes
/// [`release`] to hear of them, and its caller then calls [`finish`].
pub fn store() -> Result<&'static Store, CapError> {
    STORE.get().ok_or(CapError::NoSuchSpace)
}

/// Runs `f` with the lock of `space` held, if the space exists: the fast
/// path, for everything that follows no link of the derivation tree. What
/// `f` releases ([`release`]) is finished once the lock has gone.
pub fn in_space<R>(space: SpaceId, f: impl FnOnce(&mut Space<'_>) -> R) -> Result<R, CapError> {
    let result = store()?.in_space(space, f);
    finish();
    result
}

/// Runs `f` with the tree lock and the locks of `spaces` held (at most
/// four): the slow path, for everything that follows or changes a link.
/// What `f` releases ([`release`]) is finished once the locks have gone.
pub fn with_tree<R>(spaces: &[SpaceId], f: impl FnOnce(&mut Tree<'_>) -> R) -> Result<R, CapError> {
    let result = store()?.with_tree(spaces, f);
    finish();
    Ok(result)
}

/// [`with_tree`], with the object table's lock held too: for a caller
/// that takes a spin lock inside `f` and works on the store under it
/// (`user::ipc`, which moves a message's capabilities with an endpoint
/// locked). `f` must release nothing while it holds a spin lock.
pub fn with_all<R>(spaces: &[SpaceId], f: impl FnOnce(&mut Tree<'_>) -> R) -> Result<R, CapError> {
    let result = store()?.with_all(spaces, f);
    finish();
    Ok(result)
}

/// What follows a change of the store once its locks have gone.
/// Processes it released that have no thread left to finish them are
/// finished (`user::process`); files it released are dropped
/// (`fs::drop_deferred`), the last of a removed file freeing its space on
/// the disk; cards whose capabilities it released are stopped, which
/// takes the network stack's and address spaces' locks
/// (`user::device::close_deferred`); and the parents of the processes it
/// finished are told, which may send signals that end more processes
/// (Phase 8). Each step first looks at a flag, so a change that released
/// nothing pays four loads.
pub fn finish() {
    crate::user::process::finish_pending();
    crate::fs::drop_deferred();
    crate::user::device::close_deferred();
    crate::user::process::report_finished();
}

/// The store's version: it moves when a capability a CPU found leaves its
/// slot, and when an object a CPU found leaves the object registry
/// (`user::objects`), before the lock that guarded either goes. Read
/// before a lookup, it says the lookup's answer still holds while it
/// stays the same.
pub fn version() -> u64 {
    STORE.get().map_or(0, Store::version)
}

/// Says that a change took away something a CPU may have cached outside
/// the store (an object the registry handed out): the version moves
/// now, before the lock that guarded it goes.
pub fn mark_stale() {
    if let Some(store) = STORE.get() {
        store.bump_version();
    }
}

/// The capability in `at`, as [`Store::get`] gives it: for lookups too
/// rare to be worth a place in a CPU's cache. The system calls' lookups
/// go through `user::objects`, which keeps them.
pub fn get(at: SlotRef) -> Result<Capability, CapError> {
    store()?.get(at)
}

/// The capability in `at`, for a CPU's cache to keep (`user::objects`):
/// from now on the version moves once it leaves its slot
/// ([`Space::get_to_cache`]). With it come what `also` finds in the
/// registry, with the space's lock still held, and the version read
/// before the lookup: any change to the slot after the lookup moves it
/// past that.
pub fn get_to_cache<R>(
    at: SlotRef,
    also: impl FnOnce(&Capability, &mut Registry) -> R,
) -> Result<(Capability, R, u64), CapError> {
    let store = store()?;
    let version = store.version();
    store.in_space(at.space, |space| {
        let cap = space.get_to_cache(at)?;
        let found = space.with_registry(|registry| also(&cap, registry));
        Ok((cap, found, version))
    })?
}

/// Writes what the store holds, by kind, for `kdb`. Gives up if another
/// CPU holds one of the store's locks or the registry's.
pub fn write_census(line: &mut dyn FnMut(fmt::Arguments<'_>)) {
    let counted = STORE
        .get()
        .and_then(|store| Some((store.try_census()?, store.try_with_registry(objects::census)?)));
    let Some((census, registered)) = counted else {
        line(format_args!("Capabilities: the store is not set up, or another CPU holds a lock"));
        return;
    };
    line(format_args!(
        "Capabilities: {} spaces with {} slots, {} capabilities to {} objects, {} pages in memory \
         objects; deepest derivation {}, {} revokes under way",
        census.spaces,
        census.slots,
        census.total_caps(),
        census.total_objects(),
        census.memory_pages,
        census.deepest,
        census.revoking
    ));
    for kind in ObjectKind::ALL {
        let (caps, objects) = (census.caps[kind.index()], census.objects[kind.index()]);
        if caps > 0 || objects > 0 {
            match Some(registered[kind.index()]).filter(|&n| n > 0) {
                Some(n) => line(format_args!(
                    "  {:>13}: {caps:>6} capabilities, {objects:>6} objects, {n:>6} kernel objects",
                    kind.name()
                )),
                None => line(format_args!(
                    "  {:>13}: {caps:>6} capabilities, {objects:>6} objects",
                    kind.name()
                )),
            }
        }
    }
}

/// Gives back what the store released, with the store's locks held, the
/// object table's among them: kernel objects leave `registry` and close
/// (`user::objects`), memory objects' pages return to the frame
/// allocator, and domains leave the domain table. Every operation that
/// may release objects takes this.
pub fn release(released: Released, registry: &mut Registry) {
    if released.kind == ObjectKind::ResourceDomain {
        domains::released(released.id);
    }
    // Kernel objects close first: a frame is unmapped before its memory
    // can go back.
    objects::released(registry, &released);
    let Some(memory) = released.memory else { return };
    let order = memory.pages.trailing_zeros();
    if let Err(err) = FRAMES.free(memory.base << PAGE_SHIFT, order, Owner::Memory) {
        error!("Capabilities: could not free {} pages of {}: {err}", memory.pages, released.id);
    }
}

/// Makes a memory object of 2^`order` pages from the frame allocator and
/// puts its original capability in `at`.
pub fn create_memory(at: SlotRef, order: u32) -> Result<ObjectId, CapError> {
    let phys =
        FRAMES.alloc(order, Flags::MAY_SLEEP, Owner::Memory).map_err(|_| CapError::NoMemory)?;
    let made = store().and_then(|store| store.create_memory(at, phys >> PAGE_SHIFT, 1 << order));
    if made.is_err() {
        let _ = FRAMES.free(phys, order, Owner::Memory);
    }
    made
}

/// Makes a resource domain under `parent` and puts its original capability
/// in `at`. The domain is made before its capability is placed, so nobody
/// finds the capability without it.
pub fn create_domain(
    at: SlotRef,
    parent: &DomainRef,
    name: &str,
    settings: Settings,
) -> Result<DomainRef, CapError> {
    let kind = ObjectKind::ResourceDomain;
    let record = |id, _: &mut Registry| {
        domains::create(name, parent, settings, Some(id)).map_err(|err| match err {
            DomainError::NoMemory => CapError::NoMemory,
            DomainError::InvalidArgument | DomainError::OverLimit => CapError::InvalidArgument,
        })
    };
    let made = in_space(at.space, |space| space.create_recorded(at, kind, kind.rights(), record))??;
    Ok(made.1)
}

/// The domain the capability in `at` names, if it has `needs` on it.
pub fn domain_at(at: SlotRef, needs: Rights) -> Result<DomainRef, CapError> {
    let cap = store()?.lookup(at, ObjectKind::ResourceDomain, needs)?;
    cap.id().and_then(domains::by_object).ok_or(CapError::Empty)
}

/// Puts in `to` a capability derived from the one in `from`
/// ([`Store::derive`]).
pub fn derive(from: SlotRef, to: SlotRef, how: Derivation) -> Result<(), CapError> {
    store()?.derive(from, to, how)
}

/// Moves the capability in `from` to `to` ([`Store::move_cap`]).
pub fn move_cap(from: SlotRef, to: SlotRef) -> Result<(), CapError> {
    store()?.move_cap(from, to)
}

/// Deletes the capability in `at`.
pub fn delete(at: SlotRef) -> Result<(), CapError> {
    let deleted = store()?.delete(at, &mut release);
    finish();
    deleted
}

/// Revokes everything derived from the capability in `at`, in steps of
/// [`REVOKE_STEP`] with the locks let go in between. Returns how many
/// steps it took. A step after the first goes on only while the slot
/// keeps its revoke mark, so a capability another thread puts in the
/// slot meanwhile keeps its descendants.
pub fn revoke(at: SlotRef) -> Result<u32, CapError> {
    let store = store()?;
    let mut steps = 0;
    loop {
        steps += 1;
        let step = if steps == 1 {
            store.revoke(at, REVOKE_STEP, &mut release)
        } else {
            store.continue_revoke(at, REVOKE_STEP, &mut release)
        };
        finish();
        match step? {
            Progress::Done => return Ok(steps),
            Progress::More => crate::sched::yield_now(),
        }
    }
}

/// Destroys a capability space and everything in it.
pub fn destroy_space(space: SpaceId) -> Result<(), CapError> {
    let destroyed = store()?.destroy_space(space, &mut release);
    finish();
    destroyed
}
