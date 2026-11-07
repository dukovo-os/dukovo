// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Read-copy-update: readers that never wait or write shared memory.
//!
//! A read section ([`read_lock`], or [`RcuCell::read`]) holds off
//! preemption and nothing else, so it costs two per-CPU increments. A
//! writer publishes a new version of the data with one atomic swap and
//! frees the old one only after a *grace period*: once every read section
//! that might still see it has ended ([`synchronize`]). Radix's RCU is
//! quiescent-state based (QSBR): a CPU that has switched threads, or gone
//! idle, since the grace period started cannot be inside an older read
//! section, because read sections cannot be preempted. The kernel's
//! scheduler drives grace periods ([`Hooks::synchronize_rcu`]); see
//! `docs/design/scheduler.md`.
//!
//! Read sections must not sleep: they hold off preemption, and the
//! scheduler reports a thread that sleeps inside one.
//!
//! A writer that cannot wait, or should not (a grace period lasts until
//! every CPU has switched threads, up to a timer tick), hands what it
//! retired to [`defer`] instead: a [`Deferred`] inside it runs once the
//! grace period has passed, on the kernel's RCU thread.

use alloc::boxed::Box;
use core::alloc::Allocator;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::hooks::{self, Hooks};

/// An RCU read section; it ends when the guard is dropped. It stays on the
/// CPU it started on.
pub struct ReadGuard {
    hooks: Option<&'static dyn Hooks>,
    _not_send: PhantomData<*const ()>,
}

/// Starts a read section.
pub fn read_lock() -> ReadGuard {
    let hooks = hooks::active();
    if let Some(hooks) = hooks {
        hooks::preempt_disable(hooks);
    }
    ReadGuard { hooks, _not_send: PhantomData }
}

impl ReadGuard {
    /// The index of the CPU the read section runs on, which it stays on
    /// ([`Hooks::current_cpu`]): 0 before the kernel registers its hooks.
    #[inline]
    pub fn cpu(&self) -> usize {
        self.hooks.map_or(0, Hooks::current_cpu)
    }
}

impl Drop for ReadGuard {
    fn drop(&mut self) {
        if let Some(hooks) = self.hooks {
            hooks::preempt_enable(hooks);
        }
    }
}

/// Waits until every read section that was running when it was called has
/// ended. It sleeps, so it may not be called inside a read section or with
/// a spin lock held. Before the kernel registers its hooks only the boot
/// CPU runs, so there is nothing to wait for.
pub fn synchronize() {
    if let Some(hooks) = hooks::active() {
        hooks.synchronize_rcu();
    }
}

/// As [`synchronize`], from a caller that may not be able to sleep: true
/// once every read section that was running when it was called has ended,
/// false if it could not wait for them.
pub fn try_synchronize() -> bool {
    hooks::active().is_none_or(Hooks::try_synchronize_rcu)
}

/// A call that [`defer`] puts off until no read section can still see what
/// it frees: kept inside that, so that deferring needs no memory.
pub struct Deferred {
    /// The queue's: the next one, and the grace period it waits for.
    next: UnsafeCell<Option<NonNull<Deferred>>>,
    target: UnsafeCell<u64>,
    run: unsafe fn(NonNull<Deferred>),
}

// SAFETY: the cells are the queue's, used only by whoever owns the queue
// the call is in (`Queue`, behind the kernel's lock) or the deferring
// caller before it hands the call over.
#[allow(unsafe_code)]
unsafe impl Sync for Deferred {}
// SAFETY: as above.
#[allow(unsafe_code)]
unsafe impl Send for Deferred {}

impl Deferred {
    /// A call of `run`, given where the call is: `run` finds what holds it
    /// from there (`container_of`).
    pub const fn new(run: unsafe fn(NonNull<Deferred>)) -> Deferred {
        Deferred { next: UnsafeCell::new(None), target: UnsafeCell::new(0), run }
    }

    /// Makes the call `item` holds.
    ///
    /// # Safety
    ///
    /// As its `run` requires: once, after the grace period [`defer`]
    /// waited for.
    #[allow(unsafe_code)]
    pub unsafe fn call(item: NonNull<Deferred>) {
        // SAFETY: the caller's guarantee; `item` is live until `run`.
        unsafe { (item.as_ref().run)(item) }
    }
}

/// Deferred calls in the order they were deferred, each with the grace
/// period it waits for: the kernel keeps one, behind a lock.
pub struct Queue {
    head: Option<NonNull<Deferred>>,
    tail: Option<NonNull<Deferred>>,
}

// SAFETY: it owns the calls it holds, which are `Send`.
#[allow(unsafe_code)]
unsafe impl Send for Queue {}

impl Default for Queue {
    fn default() -> Queue {
        Queue::new()
    }
}

impl Queue {
    /// An empty queue.
    pub const fn new() -> Queue {
        Queue { head: None, tail: None }
    }

    /// Adds `item`, to run once grace period `target` has completed.
    /// Targets must not go down from one push to the next.
    ///
    /// # Safety
    ///
    /// `item` must be live and in no queue until the queue gives it back.
    #[allow(unsafe_code)]
    pub unsafe fn push(&mut self, item: NonNull<Deferred>, target: u64) {
        // SAFETY: the caller hands `item` over; a queued call's cells are
        // the queue's.
        unsafe {
            *item.as_ref().next.get() = None;
            *item.as_ref().target.get() = target;
            match self.tail {
                Some(tail) => *tail.as_ref().next.get() = Some(item),
                None => self.head = Some(item),
            }
        }
        self.tail = Some(item);
    }

    /// The grace period the first call waits for, if there is one.
    #[allow(unsafe_code)]
    pub fn first_target(&self) -> Option<u64> {
        // SAFETY: a queued call is live and its cells are the queue's.
        self.head.map(|head| unsafe { *head.as_ref().target.get() })
    }

    /// Takes out the first call, if the grace periods completed reach the
    /// one it waits for: it may run.
    #[allow(unsafe_code)]
    pub fn pop_ready(&mut self, completed: u64) -> Option<NonNull<Deferred>> {
        let head = self.head?;
        if self.first_target()? > completed {
            return None;
        }
        // SAFETY: as in `first_target`.
        self.head = unsafe { *head.as_ref().next.get() };
        if self.head.is_none() {
            self.tail = None;
        }
        Some(head)
    }
}

/// Runs the call in `item` once every read section running now has ended,
/// or later. It returns at once. It must not be called inside a read
/// section. Before the kernel registers its hooks only the boot CPU runs,
/// and the call is made now.
///
/// # Safety
///
/// `item` must stay live, and in no other queue, until the call runs; the
/// call must be safe to make then.
#[allow(unsafe_code)]
pub unsafe fn defer(item: NonNull<Deferred>) {
    match hooks::active() {
        // SAFETY: the caller's guarantee.
        Some(hooks) => unsafe { hooks.defer_rcu(item) },
        // SAFETY: as above; nothing else runs to read what it frees.
        None => unsafe { Deferred::call(item) },
    }
}

/// A heap-allocated value that readers reach without locks and writers
/// replace whole. Every value it holds is in memory from its own
/// allocator, which frees it.
pub struct RcuCell<T, A: Allocator + Clone> {
    ptr: AtomicPtr<T>,
    alloc: A,
}

// SAFETY: readers on any CPU get `&T` (so `T: Sync`), and the value is
// dropped by whichever CPU replaces or drops the cell (so `T: Send`).
unsafe impl<T: Send + Sync, A: Allocator + Clone + Send + Sync> Sync for RcuCell<T, A> {}
// SAFETY: as above.
unsafe impl<T: Send + Sync, A: Allocator + Clone + Send> Send for RcuCell<T, A> {}

impl<T, A: Allocator + Clone> RcuCell<T, A> {
    /// A cell holding `value`, in memory from `alloc`. Gives `value` back
    /// if there is no memory for it.
    pub fn new_in(value: T, alloc: A) -> Result<Self, T> {
        let ptr = boxed(value, &alloc)?;
        Ok(Self { ptr: AtomicPtr::new(ptr), alloc })
    }

    /// Runs `f` on the current value inside a read section. `f` must not
    /// sleep.
    // The cell always holds a value, so the `expect` never fails.
    #[allow(clippy::missing_panics_doc)]
    pub fn read<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let _section = read_lock();
        let ptr =
            NonNull::new(self.ptr.load(Ordering::Acquire)).expect("an RCU cell holds a value");
        // SAFETY: the pointer came from `Box::into_raw_with_allocator` and
        // is freed only after it has been swapped out and a grace period
        // has passed (`replace`) or the cell is gone (`drop`, which `&self`
        // rules out). This read section started before any such swap could
        // complete its grace period, so the value outlives `f`, and the
        // reference cannot escape `f`.
        f(unsafe { ptr.as_ref() })
    }

    /// Publishes `value` and returns the old value once no reader can see
    /// it any more. It waits for a grace period, so it sleeps: not inside a
    /// read section, and not with a spin lock held. Gives `value` back if
    /// there is no memory for it.
    pub fn replace(&self, value: T) -> Result<Box<T, A>, T> {
        let new = boxed(value, &self.alloc)?;
        let old = self.ptr.swap(new, Ordering::AcqRel);
        synchronize();
        // SAFETY: `old` came from `boxed` with a clone of this cell's
        // allocator (every value does, see `new_in`), the swap made this
        // the only caller holding it, and the grace period means no reader
        // still has a reference to it.
        Ok(unsafe { Box::from_raw_in(old, self.alloc.clone()) })
    }

    /// The current value, through `&mut`, which proves no reader or writer
    /// is active.
    pub fn get_mut(&mut self) -> &mut T {
        // SAFETY: the pointer always holds a live box (see `read`), and
        // `&mut self` rules out every other access.
        unsafe { &mut *self.ptr.load(Ordering::Relaxed) }
    }
}

/// Moves `value` into memory from `alloc`, and returns where; gives it
/// back if there is no memory.
fn boxed<T, A: Allocator + Clone>(value: T, alloc: &A) -> Result<*mut T, T> {
    match Box::try_new_uninit_in(alloc.clone()) {
        Ok(slot) => Ok(Box::into_raw_with_allocator(Box::write(slot, value)).0),
        Err(_) => Err(value),
    }
}

impl<T, A: Allocator + Clone> Drop for RcuCell<T, A> {
    fn drop(&mut self) {
        let ptr = *self.ptr.get_mut();
        // SAFETY: `&mut self` means no reader can hold a reference (they
        // borrow the cell), and the pointer is a live box from a clone of
        // this cell's allocator.
        drop(unsafe { Box::from_raw_in(ptr, self.alloc.clone()) });
    }
}

#[cfg(test)]
mod tests {
    use std::alloc::Global;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64};

    use super::*;

    /// A value whose two halves must always agree; dropping it poisons
    /// them, so a reader that sees a freed value notices.
    struct Pair {
        a: AtomicU64,
        b: AtomicU64,
    }

    impl Drop for Pair {
        fn drop(&mut self) {
            self.a.store(u64::MAX, Ordering::SeqCst);
            self.b.store(0, Ordering::SeqCst);
        }
    }

    fn pair(n: u64) -> Pair {
        Pair { a: AtomicU64::new(n), b: AtomicU64::new(n) }
    }

    #[allow(unsafe_code)]
    unsafe fn nothing(_: NonNull<Deferred>) {}

    #[test]
    fn a_queue_gives_calls_back_in_order_once_their_grace_period_is_over() {
        let calls: Vec<Deferred> = (0..3).map(|_| Deferred::new(nothing)).collect();
        let mut queue = Queue::new();
        assert_eq!(queue.first_target(), None);
        for (call, target) in calls.iter().zip([1, 1, 2]) {
            // SAFETY: each call is live, and in this queue alone.
            #[allow(unsafe_code)]
            unsafe {
                queue.push(NonNull::from(call), target);
            }
        }
        assert_eq!(queue.first_target(), Some(1));
        assert!(queue.pop_ready(0).is_none());
        assert_eq!(queue.pop_ready(1), Some(NonNull::from(&calls[0])));
        assert_eq!(queue.pop_ready(1), Some(NonNull::from(&calls[1])));
        assert!(queue.pop_ready(1).is_none());
        assert_eq!(queue.pop_ready(5), Some(NonNull::from(&calls[2])));
        assert_eq!(queue.first_target(), None);
        assert!(queue.pop_ready(5).is_none());
    }

    static DEFERRED_RAN: AtomicU64 = AtomicU64::new(0);

    #[allow(unsafe_code)]
    unsafe fn count(_: NonNull<Deferred>) {
        DEFERRED_RAN.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn a_deferred_call_runs_after_the_readers() {
        crate::testing::install();
        let call = Box::new(Deferred::new(count));
        // SAFETY: the call stays live until it runs, which the hooks'
        // default does before returning.
        #[allow(unsafe_code)]
        unsafe {
            defer(NonNull::from(&*call));
        }
        assert_eq!(DEFERRED_RAN.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn readers_never_see_freed_values() {
        crate::testing::install();
        let cell = Arc::new(RcuCell::new_in(pair(0), Global).ok().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicU64::new(0));
        let readers: Vec<_> = (0..3)
            .map(|_| {
                let (cell, stop) = (Arc::clone(&cell), Arc::clone(&stop));
                let started = Arc::clone(&started);
                std::thread::spawn(move || {
                    let mut reads = 0u64;
                    started.fetch_add(1, Ordering::SeqCst);
                    loop {
                        cell.read(|p| {
                            let a = p.a.load(Ordering::SeqCst);
                            std::thread::yield_now();
                            let b = p.b.load(Ordering::SeqCst);
                            assert_eq!(a, b, "a reader saw a value freed under it");
                        });
                        reads += 1;
                        if stop.load(Ordering::Relaxed) {
                            break reads;
                        }
                    }
                })
            })
            .collect();
        while started.load(Ordering::SeqCst) < 3 {
            std::thread::yield_now();
        }
        for n in 1..200 {
            let old = cell.replace(pair(n)).ok().unwrap();
            assert_eq!(old.a.load(Ordering::SeqCst), n - 1);
        }
        stop.store(true, Ordering::Relaxed);
        let reads: u64 = readers.into_iter().map(|r| r.join().unwrap()).sum();
        assert!(reads >= 3);
        cell.read(|p| assert_eq!(p.a.load(Ordering::SeqCst), 199));
    }

    #[test]
    fn get_mut_and_drop() {
        let mut cell = RcuCell::new_in(pair(5), Global).ok().unwrap();
        *cell.get_mut().a.get_mut() = 6;
        cell.read(|p| assert_eq!(p.a.load(Ordering::Relaxed), 6));
    }
}
