// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel objects: what each object name the capability store hands
//! out stands for.
//!
//! The store keeps capabilities and the derivation tree, and knows each
//! object by its kind and an [`ObjectId`]. The objects themselves (an
//! endpoint's queues, a frame's mappings) are kept in the store's record
//! of each object ([`Recorded`]), behind that record's own lock (ADR 0108,
//! ADR 0369). An object is recorded when the kernel makes it, after the
//! store gives it a name and before its capability is placed
//! ([`create`]), so nobody finds the capability without it. It leaves
//! when the store releases the name (its last capability is gone), which
//! also closes it ([`released`]): an endpoint fails its waiting threads,
//! a frame is unmapped from every address space. Holders of a reference
//! (a thread blocked on an endpoint, a region of an address space) keep
//! the memory alive until they let go.
//!
//! Making or releasing an object takes its space's lock and its record's,
//! and no lock other objects share. A system call finds a capability and
//! the object it designates with one look at its CPU's cache, which takes
//! no lock when this CPU found that slot before at the version the store
//! has now ([`find`]): system calls on different CPUs share no lock to
//! reach their objects. A capability a CPU found leaving its slot, or an
//! object a CPU found leaving the store, moves the version (`caps`); one
//! nobody looked up, such as a file opened and closed again, does not.
//!
//! A call on a file or directory capability of a process with one thread
//! borrows the object without counting a reference, as Linux's light
//! descriptor lookup does ([`find_holding`], ADR 0414): nobody but that
//! thread can empty the slot while the call runs, so the store's reference
//! keeps the object. Before its CPU passes an RCU quiescent state or
//! switches threads, the borrow is counted ([`count_borrowed`]), so a
//! revoke from a program, or a thread another process starts in this one,
//! waits for a grace period before it can empty the slot.

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::mem::ManuallyDrop;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use radix_abi::errors::Kernel;
use radix_cap::{CapError, Capability, ObjectId, ObjectKind, Rights, SlotRef, SpaceId};
use radix_hal::{Cpu, CpuLocal, MAX_CPUS};
use radix_mm::{Flags, HEAP, Kmalloc};

use super::account::Charge;
use super::device::{DeviceObject, DeviceRef};
use super::ipc::{Endpoint, EndpointRef};
use super::memory::{FrameObject, FrameRef, SpaceObject, SpaceRef};
use super::notify::{Notification, NotificationRef};
use super::process::{ProcessObject, ProcessRef};
use super::ring::{RingObject, RingRef};
use super::{Result, cap_error, error};
use crate::Arch;
use crate::caps;
use crate::fs::{FileObject, FileRef};
use crate::percpu::{Own, PerCpu, Slot};
use crate::sched::{Thread, ThreadRef};

/// A kernel object.
#[derive(Clone)]
pub enum Object {
    /// An IPC endpoint.
    Endpoint(EndpointRef),
    /// A notification.
    Notification(NotificationRef),
    /// A thread.
    Thread(ThreadRef),
    /// An address space.
    Space(SpaceRef),
    /// A frame.
    Frame(FrameRef),
    /// A ring.
    Ring(RingRef),
    /// A process.
    Process(ProcessRef),
    /// The kernel's random number generator.
    Random,
    /// The kernel log.
    KernelLog,
    /// A file (radix-file(2)).
    File(FileRef),
    /// A directory (radix-directory(2)).
    Directory(FileRef),
    /// The machine's power (radix-power(2)).
    Power,
    /// The machine's platform controls (radix-platform(2)).
    Platform,
    /// The authority to give names (radix-names(2)).
    Names,
    /// The time of day and the real-time clock (radix-clock(2)).
    Clock,
    /// The cards the kernel left for driver programs (radix-device(2)).
    Devices,
    /// A card a driver program drives.
    Device(DeviceRef),
    /// The firewall's rules (radix-firewall(2)).
    Firewall,
}

impl Object {
    /// The kind of object it is.
    pub fn kind(&self) -> ObjectKind {
        match self {
            Object::Endpoint(_) => ObjectKind::Endpoint,
            Object::Notification(_) => ObjectKind::Notification,
            Object::Thread(_) => ObjectKind::Thread,
            Object::Space(_) => ObjectKind::AddressSpace,
            Object::Frame(_) => ObjectKind::Frame,
            Object::Ring(_) => ObjectKind::Ring,
            Object::Process(_) => ObjectKind::Process,
            Object::Random => ObjectKind::Random,
            Object::KernelLog => ObjectKind::KernelLog,
            Object::File(_) => ObjectKind::File,
            Object::Directory(_) => ObjectKind::Directory,
            Object::Power => ObjectKind::Power,
            Object::Platform => ObjectKind::Platform,
            Object::Names => ObjectKind::Names,
            Object::Clock => ObjectKind::Clock,
            Object::Devices | Object::Device(_) => ObjectKind::Device,
            Object::Firewall => ObjectKind::Firewall,
        }
    }
}

/// What the store keeps of a kernel object, in the object's record
/// (`radix_cap`'s `K`), which the store checks against the object's name
/// and locks around each look.
pub struct Recorded {
    /// A CPU's cache may hold it ([`get_to_cache`]): its going moves the
    /// store's version.
    cached: bool,
    object: Object,
    /// What the object costs, charged to the account of the process that
    /// made it (`process.create`), and given back when it goes.
    charge: Option<Charge>,
}

/// Records `object` in `kept`, the store's place for it in the record of
/// an object it has just named, with what it was charged, which is given
/// back when the object goes: from the store's `record` callback, before
/// the object's capability is placed, so that nobody finds the capability
/// without its object. `object` is taken only if it is recorded; on a
/// failure it is left to the caller, to drop once the store's locks have
/// gone, since it may hold the last reference to something whose drop
/// takes locks (a removed file frees its space on the disk).
pub fn record(
    kept: &mut Option<Recorded>,
    object: &mut Option<Object>,
    charge: Option<Charge>,
) -> core::result::Result<(), CapError> {
    let Some(object) = object.take() else { return Err(CapError::Empty) };
    *kept = Some(Recorded { cached: false, object, charge });
    Ok(())
}

/// Makes an object of `kind` in the store, with its original capability,
/// holding only `rights`, in `at`, and records `object` with it with what
/// it was charged ([`record`]), with the lock of `at`'s space held
/// throughout. `object` is taken only once nothing can fail; on a failure
/// it is left to the caller, as [`record`] leaves it.
pub fn create(
    at: SlotRef,
    kind: ObjectKind,
    rights: Rights,
    object: &mut Option<Object>,
    charge: Option<Charge>,
) -> Result<ObjectId> {
    caps::in_space(at.space, |space| {
        let record = |_, kept: &mut Option<Recorded>| record(kept, object, charge);
        space.create_recorded(at, kind, rights, record).map(|(id, ())| id)
    })
    .and_then(|made| made)
    .map_err(cap_error)
}

/// The object named `id`, if it lives and is a kernel object, looked up
/// under its record's lock: for lookups too rare to be worth a place in a
/// CPU's cache. The system calls' lookups go through [`find`].
pub fn get(id: ObjectId) -> Option<Object> {
    caps::store().ok()?.with_kept(id, |kept| kept.object.clone())
}

/// The object `kept` holds, for a CPU's cache to keep: from now on the
/// version moves once it goes. Called under its record's lock, with the
/// lock of the space whose slot named it held too (`caps::get_to_cache`).
fn get_to_cache(kept: &mut Recorded) -> Object {
    kept.cached = true;
    kept.object.clone()
}

/// An object as a CPU's cache keeps it: a weak reference, which keeps its
/// memory but not the object, so a cache never holds on to an object the
/// store has let go of. A hit takes a strong reference again, which
/// fails once the object is gone.
enum Kept {
    Endpoint(Weak<Endpoint, Kmalloc>),
    Notification(Weak<Notification, Kmalloc>),
    Thread(Weak<Thread, Kmalloc>),
    Space(Weak<SpaceObject, Kmalloc>),
    Frame(Weak<FrameObject, Kmalloc>),
    Ring(Weak<RingObject, Kmalloc>),
    Process(Weak<ProcessObject, Kmalloc>),
    Random,
    KernelLog,
    File(Weak<FileObject, Kmalloc>),
    Directory(Weak<FileObject, Kmalloc>),
    Power,
    Platform,
    Names,
    Clock,
    Devices,
    Device(Weak<DeviceObject, Kmalloc>),
    Firewall,
}

impl Kept {
    fn of(object: &Object) -> Kept {
        match object {
            Object::Endpoint(endpoint) => Kept::Endpoint(Arc::downgrade(endpoint)),
            Object::Notification(notification) => Kept::Notification(Arc::downgrade(notification)),
            Object::Thread(thread) => Kept::Thread(Arc::downgrade(thread)),
            Object::Space(space) => Kept::Space(Arc::downgrade(space)),
            Object::Frame(frame) => Kept::Frame(Arc::downgrade(frame)),
            Object::Ring(ring) => Kept::Ring(Arc::downgrade(ring)),
            Object::Process(process) => Kept::Process(Arc::downgrade(process)),
            Object::Random => Kept::Random,
            Object::KernelLog => Kept::KernelLog,
            Object::File(file) => Kept::File(Arc::downgrade(file)),
            Object::Directory(dir) => Kept::Directory(Arc::downgrade(dir)),
            Object::Power => Kept::Power,
            Object::Platform => Kept::Platform,
            Object::Names => Kept::Names,
            Object::Clock => Kept::Clock,
            Object::Devices => Kept::Devices,
            Object::Device(device) => Kept::Device(Arc::downgrade(device)),
            Object::Firewall => Kept::Firewall,
        }
    }

    #[inline(always)]
    fn upgrade(&self) -> Option<Object> {
        Some(match self {
            Kept::Endpoint(endpoint) => Object::Endpoint(endpoint.upgrade()?),
            Kept::Notification(notification) => Object::Notification(notification.upgrade()?),
            Kept::Thread(thread) => Object::Thread(thread.upgrade()?),
            Kept::Space(space) => Object::Space(space.upgrade()?),
            Kept::Frame(frame) => Object::Frame(frame.upgrade()?),
            Kept::Ring(ring) => Object::Ring(ring.upgrade()?),
            Kept::Process(process) => Object::Process(process.upgrade()?),
            Kept::Random => Object::Random,
            Kept::KernelLog => Object::KernelLog,
            Kept::File(file) => Object::File(file.upgrade()?),
            Kept::Directory(dir) => Object::Directory(dir.upgrade()?),
            Kept::Power => Object::Power,
            Kept::Platform => Object::Platform,
            Kept::Names => Object::Names,
            Kept::Clock => Object::Clock,
            Kept::Devices => Object::Devices,
            Kept::Device(device) => Object::Device(device.upgrade()?),
            Kept::Firewall => Object::Firewall,
        })
    }
}

/// Capabilities a CPU keeps, with their objects, in a table indexed by
/// slot.
const CACHED: usize = 16;

/// A capability this CPU found in a slot, the object it designates, and
/// the store's version then.
struct Cached {
    at: SlotRef,
    version: u64,
    cap: Capability,
    /// `None` if the capability designates no kernel object, as a memory
    /// capability does.
    object: Option<Kept>,
}

/// Each CPU's capabilities, which only its own CPU uses.
static CACHE: PerCpu<Own<[Option<Cached>; CACHED]>> =
    PerCpu::new([const { Slot::new(Own::new([const { None }; CACHED])) }; MAX_CPUS]);

fn cache_index(at: SlotRef) -> usize {
    (at.slot as usize ^ at.space.index as usize) % CACHED
}

/// A capability, and the object it designates, as [`find`] finds them.
pub struct Found {
    /// The capability.
    pub cap: Capability,
    /// The object it designates, if the caller wanted it and it is a
    /// kernel object, unless `held`.
    pub object: Option<Object>,
    /// Whether the capability designates the process the caller said it
    /// holds ([`find_holding`]), which `object` then leaves out.
    pub held: bool,
    /// Whether `object` is borrowed without a reference of its own
    /// ([`find_holding`]), which only a [`Lent`] holds.
    borrowed: bool,
}

/// Where [`find_holding`] puts what a system call finds, which the call
/// may borrow without a reference of its own, and gives back when it ends
/// ([`Lent::end`]). Only this holds a borrowed [`Found`], and lends it out
/// shared, so nothing can take the uncounted object out of it; a plain
/// [`Found`], which every other lookup makes, has nothing to give back
/// and drops as it is. Dropped without `end`, it would forget what it
/// holds: a leak, never a reference dropped twice.
#[derive(Default)]
pub struct Lent(ManuallyDrop<Option<Found>>);

impl Lent {
    /// Ends the call: gives back what it borrowed ([`give_back`]), then
    /// drops what it found. The system call ends each call with this, at
    /// the one place every way out of it passes.
    #[inline(always)]
    pub fn end(mut self) {
        give_back(&mut self.0);
        // Dropped where it is, rather than moved out to drop.
        *self.0 = None;
    }
}

/// Gives back the object a call borrowed ([`find_holding`]), as the call
/// ends: one the scheduler counted meanwhile ([`count_borrowed`]) stays
/// in `found`, to drop that reference as any other `object` does; one it
/// did not is taken out and forgotten, as nothing counted it.
#[inline(always)]
fn give_back(found: &mut Option<Found>) {
    if let Some(found) = found
        && found.borrowed
        && !counted_since()
    {
        // Written over without being read or dropped.
        core::mem::forget(found.object.take());
    }
}

/// The file or directory object the thread running on each CPU borrowed
/// without counting a reference ([`find_holding`]), or null. Only its own
/// CPU reads or writes it: the thread, with preemption off, and the
/// scheduler, with interrupts off and the thread's preemption count at
/// zero or the thread leaving the CPU ([`count_borrowed`]), so plain loads
/// and stores do.
static BORROWED: PerCpu<AtomicPtr<FileObject>> =
    PerCpu::new([const { Slot::new(AtomicPtr::new(core::ptr::null_mut())) }; MAX_CPUS]);

/// Revokes programs asked for that are under way ([`revoking`]): while
/// there is one, calls count their references.
static REVOKES: AtomicUsize = AtomicUsize::new(0);

/// The capability in slot `slot` of `cspace`, and the object it
/// designates if `wanted` says its kind's is needed, found in `place`.
///
/// If this CPU found the capability at the version the store has now, it
/// has not left the slot since, nor its object the store, and nothing
/// else changes either: no lock, and nothing shared written but the
/// object's reference count. A change under way that has not moved the
/// version yet has not returned either, so a lookup that misses it
/// happened before it. Otherwise the store is asked under the space's
/// lock and the object's record's (`caps::get_to_cache`,
/// [`get_to_cache`], so that the version moves once either goes), and the
/// answer is kept with the version read before the store was asked: a
/// change after the lookup moves the version past it.
///
/// What is found is written in the caller's `place`, not returned: a
/// capability and an object are 88 bytes, which a move would copy with a
/// string move, whose start-up costs more than the lookup.
#[inline(always)]
pub fn find(
    cspace: SpaceId,
    slot: u32,
    wanted: impl Fn(ObjectKind) -> bool,
    place: &mut Option<Found>,
) -> Result<&mut Found> {
    find_in(cspace, slot, wanted, None, place)
}

/// As [`find`], for a system call of a thread of `process`, which holds a
/// reference to it and names the slot in its space, `cspace`. What it
/// finds is only lent: the call uses it and lets it go.
///
/// If this CPU's cache finds the capability designating `process`, the
/// lookup takes no second reference (two atomic read-modify-writes on a
/// line every thread of the process shares) but says it is the one held,
/// in [`Found::held`]. A POSIX program's system calls are methods of its
/// own process. That is sound because the reference keeps the process's
/// memory where it is: the cache's weak reference to the same address is
/// to the same object, which the capability designated at the version
/// the store has now.
///
/// If it finds a file or directory capability, and the thread is its
/// process's only one, and no revoke a program asked for is under way, it
/// borrows the object without counting a reference either, as Linux's
/// light descriptor lookup does when no other thread shares the table (ADR
/// 0414). The capability keeps the store's reference, and nobody but the
/// thread can empty its slot while the call runs: the process's other
/// threads would have to exist, a revoke waits for a grace period first
/// ([`revoking`]), and so does a thread another process starts in this
/// one (`process::ProcessObject::start`). The borrow is counted before
/// this CPU passes a quiescent state or switches threads
/// ([`count_borrowed`]), so a grace period that starts after the version
/// was read ends only once the borrow holds a reference of its own. All
/// that is decided in one read section, with preemption off: no quiescent
/// state can come between the version read and the borrow recorded.
#[inline(always)]
pub fn find_holding<'a>(
    cspace: SpaceId,
    slot: u32,
    wanted: impl Fn(ObjectKind) -> bool,
    process: &ProcessRef,
    place: &'a mut Lent,
) -> Result<&'a Found> {
    find_in(cspace, slot, wanted, Some(process), &mut place.0).map(|found| &*found)
}

/// [`find`], or [`find_holding`] with the caller's own `process`.
#[inline(always)]
fn find_in<'a>(
    cspace: SpaceId,
    slot: u32,
    wanted: impl Fn(ObjectKind) -> bool,
    process: Option<&ProcessRef>,
    place: &'a mut Option<Found>,
) -> Result<&'a mut Found> {
    let at = SlotRef::new(cspace, slot);
    let index = cache_index(at);
    'cached: {
        let Some(cache) = CACHE.own() else { break 'cached };
        // Read with preemption off, for a borrow: see `find_holding`.
        let now = caps::version();
        let Some(entry) = &cache[index] else { break 'cached };
        if entry.at != at || entry.version != now {
            break 'cached;
        }
        let object = match &entry.object {
            Some(Kept::Process(kept))
                if process.is_some_and(|held| core::ptr::eq(kept.as_ptr(), Arc::as_ptr(held)))
                    && wanted(entry.cap.kind()) =>
            {
                let found = Found { cap: entry.cap, object: None, held: true, borrowed: false };
                return Ok(place.insert(found));
            }
            Some(Kept::File(file)) if lends(process) && wanted(entry.cap.kind()) => {
                return Ok(place.insert(borrow(entry.cap, file, false)));
            }
            Some(Kept::Directory(dir)) if lends(process) && wanted(entry.cap.kind()) => {
                return Ok(place.insert(borrow(entry.cap, dir, true)));
            }
            Some(kept) if wanted(entry.cap.kind()) => match kept.upgrade() {
                Some(object) => Some(object),
                // Gone, so the version is about to move: ask the store.
                None => break 'cached,
            },
            _ => None,
        };
        return Ok(place.insert(Found { cap: entry.cap, object, held: false, borrowed: false }));
    }
    let found = place.insert(fill(at, index)?);
    if !wanted(found.cap.kind()) {
        found.object = None;
    }
    Ok(found)
}

/// Whether a call of a thread of `process`, its own, may borrow a file
/// or directory object without counting a reference ([`find_holding`]):
/// with preemption off, in the read section that found the capability.
#[inline(always)]
fn lends(process: Option<&ProcessRef>) -> bool {
    process.is_some_and(|own| own.thread_count() == 1) && REVOKES.load(Ordering::Relaxed) == 0
}

/// Borrows the file or directory object `kept` holds, which capability
/// `cap` in this CPU's cache designates, without counting a reference
/// (ADR 0414), and records the borrow for the scheduler to count. With
/// preemption off, in the read section that found the capability at the
/// store's version ([`find_holding`]).
#[inline(always)]
fn borrow(cap: Capability, kept: &Weak<FileObject, Kmalloc>, directory: bool) -> Found {
    let at = BORROWED.this();
    debug_assert!(at.load(Ordering::Relaxed).is_null(), "a CPU borrowed two objects at once");
    at.store(kept.as_ptr().cast_mut(), Ordering::Relaxed);
    // SAFETY: the cache's weak reference keeps the memory, and the
    // capability, found in its slot at the store's version, keeps the
    // store's reference to the object: nobody but this thread empties the
    // slot before this CPU passes a quiescent state, and before that the
    // borrow is counted (`find_holding`). The `Arc` made here is dropped
    // only if it was counted; otherwise it is forgotten (`give_back`).
    #[allow(unsafe_code)]
    let file = unsafe { Arc::from_raw_in(kept.as_ptr(), *Weak::allocator(kept)) };
    let object = if directory { Object::Directory(file) } else { Object::File(file) };
    Found { cap, object: Some(object), held: false, borrowed: true }
}

/// Ends the running thread's borrow ([`borrow`]) as its call ends, and
/// says whether the scheduler counted it meanwhile ([`count_borrowed`]),
/// on this CPU or one it ran on before.
#[inline(always)]
fn counted_since() -> bool {
    // With interrupts off, the thread stays on this CPU and no quiescent
    // state can count the borrow in between: a few instructions, where
    // letting preemption back on would check for a reschedule.
    let were = Arch::disable_interrupts();
    let at = BORROWED.this();
    let borrowed = at.load(Ordering::Relaxed);
    at.store(core::ptr::null_mut(), Ordering::Relaxed);
    Arch::restore_interrupts(were);
    borrowed.is_null()
}

/// Counts the object the thread running on CPU `cpu` borrowed without
/// counting a reference, if it did ([`find_holding`], ADR 0414). The
/// scheduler calls it with interrupts off on that CPU, before the CPU
/// passes a quiescent state and before it switches threads, while the
/// thread's preemption count is zero or the thread is leaving the CPU: the
/// store's reference still keeps the object, so the count is above zero.
/// Its call drops the reference when it ends ([`give_back`]), on whatever
/// CPU it then runs.
#[inline(always)]
pub fn count_borrowed(cpu: usize) {
    let at = BORROWED.get(cpu);
    let borrowed = at.load(Ordering::Relaxed);
    if !borrowed.is_null() {
        at.store(core::ptr::null_mut(), Ordering::Relaxed);
        count(borrowed);
    }
}

#[cold]
#[inline(never)]
fn count(file: *const FileObject) {
    // SAFETY: a borrow's object, live as `count_borrowed` says. Counting
    // takes no memory, so the allocator given is never used.
    #[allow(unsafe_code)]
    unsafe {
        Arc::increment_strong_count_in(file, HEAP.with(Flags::ATOMIC));
    }
}

/// Whether the running thread borrows `file` ([`borrow`]).
fn borrowed_here(file: &FileRef) -> bool {
    Arch::preempt_disable();
    let borrowed = BORROWED.this().load(Ordering::Relaxed);
    crate::sched::preempt_enable();
    core::ptr::eq(borrowed, Arc::as_ptr(file))
}

/// Runs `revoke`, a revoke a program asked for, once every call that
/// borrowed an object without counting a reference has counted one or
/// ended (ADR 0414): a revoke deletes capabilities in other processes'
/// spaces, which their calls may be borrowing from. Calls that start while
/// it runs count their references.
pub fn revoking<R>(revoke: impl FnOnce() -> R) -> R {
    REVOKES.fetch_add(1, Ordering::SeqCst);
    crate::sched::rcu::synchronize();
    let done = revoke();
    REVOKES.fetch_sub(1, Ordering::Release);
    done
}

/// The object the capability in slot `slot` of `cspace` designates, if
/// it is a kernel object: [`find`] for a caller that wants only the
/// object, which leaves the capability where it is rather than copy it.
#[inline(always)]
pub fn find_object(cspace: SpaceId, slot: u32) -> Option<Object> {
    let at = SlotRef::new(cspace, slot);
    let now = caps::version();
    let index = cache_index(at);
    'cached: {
        let Some(cache) = CACHE.own() else { break 'cached };
        let Some(entry) = &cache[index] else { break 'cached };
        if entry.at != at || entry.version != now {
            break 'cached;
        }
        return match &entry.object {
            Some(kept) => match kept.upgrade() {
                Some(object) => Some(object),
                // Gone, so the version is about to move: ask the store.
                None => break 'cached,
            },
            None => None,
        };
    }
    fill(at, index).ok()?.object
}

/// [`find`] when this CPU's cache cannot answer: kept apart, so that a
/// hit needs neither its registers nor its stack.
#[cold]
#[inline(never)]
fn fill(at: SlotRef, index: usize) -> Result<Found> {
    let (cap, object, version) = caps::get_to_cache(at, get_to_cache).map_err(cap_error)?;
    let kept = Cached { at, version, cap, object: object.as_ref().map(Kept::of) };
    // What the entry held before goes once preemption is back on: it may
    // be the last reference to an object's memory.
    let before = CACHE.with_own(|cache| cache[index].replace(kept));
    drop(before);
    Ok(Found { cap, object, held: false, borrowed: false })
}

/// The capability in slot `slot` of `cspace`, if it may be used on a
/// `kind` with the rights `needs`, and the object it designates.
pub fn lookup(
    cspace: SpaceId,
    slot: u32,
    kind: ObjectKind,
    needs: Rights,
) -> Result<(Capability, Object)> {
    let mut place = None;
    let found = find(cspace, slot, |of| of == kind, &mut place)?;
    found.cap.check(kind, needs).map_err(cap_error)?;
    // Released since the capability was found: as good as gone.
    let object = found.object.take().ok_or(error(Kernel::Gone))?;
    Ok((found.cap, object))
}

/// The capability in slot `slot` of `cspace`, whatever it designates.
pub fn capability(cspace: SpaceId, slot: u32) -> Result<Capability> {
    let mut place = None;
    Ok(find(cspace, slot, |_| false, &mut place)?.cap)
}

/// The object `cap`, looked up a moment ago, designates, through [`get`].
pub fn object_of(cap: &Capability) -> Result<Object> {
    // Released since the lookup: as good as gone.
    cap.id().and_then(get).ok_or(error(Kernel::Gone))
}

macro_rules! typed_lookup {
    ($(#[$doc:meta])* $name:ident, $kind:ident, $variant:ident, $type:ty) => {
        $(#[$doc])*
        pub fn $name(cspace: SpaceId, slot: u32, needs: Rights) -> Result<(Capability, $type)> {
            match lookup(cspace, slot, ObjectKind::$kind, needs)? {
                (cap, Object::$variant(object)) => Ok((cap, object)),
                _ => Err(error(Kernel::Gone)),
            }
        }
    };
}

typed_lookup!(
    /// The endpoint a capability designates.
    endpoint,
    Endpoint,
    Endpoint,
    EndpointRef
);
typed_lookup!(
    /// The notification a capability designates.
    notification,
    Notification,
    Notification,
    NotificationRef
);
typed_lookup!(
    /// The address space a capability designates.
    space,
    AddressSpace,
    Space,
    SpaceRef
);
typed_lookup!(
    /// The frame a capability designates.
    frame,
    Frame,
    Frame,
    FrameRef
);
typed_lookup!(
    /// The ring a capability designates.
    ring,
    Ring,
    Ring,
    RingRef
);
typed_lookup!(
    /// The thread a capability designates.
    thread,
    Thread,
    Thread,
    ThreadRef
);
typed_lookup!(
    /// The file a capability designates.
    file,
    File,
    File,
    FileRef
);
typed_lookup!(
    /// The directory a capability designates.
    directory,
    Directory,
    Directory,
    FileRef
);

/// The store released an object, and gave back what it kept of it: the
/// object is closed, with no lock of its record's held, and the store's
/// other locks held or not (`caps::release`). If a CPU's cache may hold
/// it, the version moves first, before the operation that released it
/// returns; its index is not handed out again until then either. What it
/// was charged goes back to the stock of the thread that released it,
/// `by`, if it was charged to that thread's process, and to its account
/// otherwise.
pub fn released(kept: Option<Recorded>, by: Option<&super::UserThread>) {
    let Some(Recorded { cached, object, charge }) = kept else { return };
    if cached {
        caps::mark_stale();
    }
    if cfg!(debug_assertions)
        && let (Some(_), Object::File(file) | Object::Directory(file)) = (by, &object)
    {
        // The thread's own call never empties the slot it borrows from.
        assert!(!borrowed_here(file), "a call released the object it borrowed");
    }
    match object {
        Object::Endpoint(endpoint) => endpoint.close(),
        Object::Notification(notification) => notification.close(),
        Object::Frame(frame) => super::memory::FrameObject::close(&frame),
        Object::Ring(ring) => super::ring::RingObject::close(&ring),
        // With its last capability gone, nobody can control the thread:
        // it ends at its next return to user mode.
        Object::Thread(thread) => {
            thread.request_stop();
            crate::sched::wake(&thread);
        }
        // It ends; with no thread left to finish it, the change's caller
        // does once the store's locks have gone (`process::finish_pending`).
        Object::Process(process) => super::process::released(process),
        // It may be the last reference to a file removed while open,
        // whose space is freed on the disk when it goes: that waits until
        // the store's locks have gone (`fs::release`, `fs::drop_deferred`).
        Object::File(file) | Object::Directory(file) => crate::fs::release(file),
        // Its card stops once the store's locks have gone
        // (`device::close_deferred`).
        Object::Device(device) => super::device::released(&device),
        Object::Space(_)
        | Object::Random
        | Object::KernelLog
        | Object::Power
        | Object::Platform
        | Object::Names
        | Object::Clock
        | Object::Devices
        | Object::Firewall => {}
    }
    // What it was charged is given back once it has closed.
    match (charge, by) {
        (Some(charge), Some(by)) => {
            debug_assert!(super::runs(by), "a stock touched by another thread than its own");
            by.stock.take_back(by.process.account(), charge);
        }
        (charge, _) => drop(charge),
    }
}

/// Extra room [`gather`] reserves, for processes made while it counts.
const ROOM: usize = 8;

/// What `pick` takes from each process the store keeps, gathered one
/// record's lock at a time: the processes are counted first, so the room
/// is reserved with no lock held, and then gathered. If more were made in
/// between than the extra room holds, it starts again, up to a few times,
/// then gives what it has. `None` if there is no memory for them.
fn gather<T>(pick: impl Fn(&ProcessRef) -> T) -> Option<Vec<T, Kmalloc>> {
    let store = caps::store().ok()?;
    let mut gathered = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    for _ in 0..4 {
        let mut count = 0;
        store.each_kept(|_, kept| count += usize::from(matches!(kept.object, Object::Process(_))));
        gathered.clear();
        gathered.try_reserve_exact(count + ROOM).ok()?;
        let mut all = true;
        store.each_kept(|_, kept| {
            let Object::Process(process) = &kept.object else { return };
            // Checked first, so nothing picked is dropped under the lock.
            if gathered.len() < gathered.capacity() {
                let _ = gathered.push_within_capacity(pick(process));
            } else {
                all = false;
            }
        });
        if all {
            break;
        }
    }
    Some(gathered)
}

/// The numbers of the processes the store keeps, for `/processes`.
pub fn process_numbers() -> Vec<u64, Kmalloc> {
    gather(|process| process.number()).unwrap_or_else(|| Vec::new_in(HEAP.with(Flags::MAY_SLEEP)))
}

/// The processes the store keeps, taken out so that what is done with
/// them takes no lock of theirs under the store's (radix-names(2)'s
/// `end`); none if there is no memory for them.
pub fn processes() -> Option<Vec<ProcessRef, Kmalloc>> {
    gather(ProcessRef::clone)
}

/// The process numbered `number`, if the store keeps it.
pub fn process_by_number(number: u64) -> Option<ProcessRef> {
    let mut found = None;
    caps::store().ok()?.each_kept(|_, kept| match &kept.object {
        Object::Process(process) if found.is_none() && process.number() == number => {
            found = Some(process.clone());
        }
        _ => {}
    });
    found
}

/// How many kernel objects of each kind the store keeps, for `kdb` and
/// the user tests. If not `wait`, it gives up (`None`) rather than wait
/// for a lock: a debugger may have stopped the CPU that holds one.
#[inline(never)]
pub fn census(store: &caps::Store, wait: bool) -> Option<[u32; radix_cap::KINDS]> {
    let mut counts = [0; radix_cap::KINDS];
    let mut count = |_, kept: &mut Recorded| counts[kept.object.kind().index()] += 1;
    if wait {
        store.each_kept(&mut count);
    } else {
        store.try_each_kept(&mut count)?;
    }
    Some(counts)
}
