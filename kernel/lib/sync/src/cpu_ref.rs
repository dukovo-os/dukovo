// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! References counted on each CPU (ADR 0404), as Linux counts a mount's
//! (`mnt_pcp`) and its `percpu_ref`s.
//!
//! An `Arc`'s count is one word, which every clone and every drop
//! changes: CPUs that make and drop references to one value at once pass
//! its cache line between them, about a hundred nanoseconds each time. A
//! mount is such a value: every file opened on it holds a reference.
//!
//! A [`CpuRef`] is an `Arc` whose value can also be counted on the CPUs
//! ([`CpuRef::count_on_cpus`]). The value then has a *line* of the table
//! of counts, which has a row for each CPU, and a reference made adds one
//! to its line's count in the row of the CPU that makes it, and takes one
//! from the count in the row of the CPU that drops it, which may be
//! another: no CPU writes another's row on the way. Each reference
//! remembers which way it was counted. The counts hold one reference of
//! the `Arc`'s between them, so a reference counted on the CPUs, which
//! the `Arc` does not count, cannot outlive the value.
//!
//! A clone or a drop counted on the CPUs runs inside an RCU read section
//! that first sees the value still counted there. In the kernel that is
//! `radix_count_here`, which the kernel defines and fat LTO inlines into
//! every clone and drop: it holds off interrupts around the count, which
//! no grace period passes, and finds the CPU in one load, where a read
//! section through the hooks costs three calls through pointers. A row is
//! changed only by its own CPU, held so (the slow way's drops too), so
//! the kernel changes a count with a plain load and store, not an atomic
//! add (ADR 0406). Elsewhere, as in the host's tests, it is a read
//! section through the hooks, and an atomic add, as threads share CPU
//! numbers there. To know how many references there are, the owner pauses the
//! counting
//! ([`CpuRef::pause`]) and waits for a grace period
//! ([`crate::rcu::synchronize`]): every clone and drop that saw the old
//! state has finished then, and the ones after it go the slow way, which
//! changes the counts under a lock and makes new references the `Arc`'s.
//! The counts added up, and the `Arc`'s count, are then exact
//! ([`CpuRef::references`]). The owner either goes back to counting on
//! the CPUs ([`CpuRef::resume`]) or stops for good ([`CpuRef::stop`]):
//! from then on each drop of a reference counted on the CPUs adds the
//! counts up under the lock, and the one that brings them to none gives
//! the line back, all zero, and drops the `Arc` reference the counts
//! held. `kernel/fs/vfs/tla/MountRefs.tla` checks this.
//!
//! A count may wrap, as one CPU may make references that others drop.
//! The counts are added up wrapping, and their sum, the references there
//! are, is always far below 2^31.

use alloc::sync::Arc;
use core::alloc::AllocatorClone;
use core::mem::ManuallyDrop;
use core::ops::Deref;
use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

use crate::SpinLock;
use crate::cpu::HeldCpu;

/// Values that can be counted on the CPUs at once: the lines of the table.
/// A value counted when all are taken stays the `Arc`'s.
pub const LINES: usize = 64;

/// The rows of the table, one for each CPU the kernel can run.
pub const CPUS: usize = crate::cpu::CPUS;

/// One CPU's counts, a line of 256 bytes, so that no two CPUs' rows share
/// a cache line.
#[repr(align(64))]
struct Row([AtomicU32; LINES]);

/// The counts: 64 KiB.
static COUNTS: [Row; CPUS] = [const { Row([const { AtomicU32::new(0) }; LINES]) }; CPUS];

/// Which lines are taken: bit `n` for line `n`.
static TAKEN: AtomicU64 = AtomicU64::new(0);

/// The slow way's lock: over every change of a count once the value it
/// is for is not counted on the CPUs, and over adding the counts up.
static SLOW: SpinLock<()> = SpinLock::new(());

/// Counted by the `Arc` alone: never on the CPUs, or no line was free.
const ARC: u8 = 0;
/// Counted on the CPUs.
const ON_CPUS: u8 = 1;
/// Paused: new references are the `Arc`'s, and dropped ones that were
/// counted on the CPUs change the counts under the slow lock.
const PAUSED: u8 = 2;
/// Stopped for good: as paused, and the drop that brings the counts to
/// none gives the line back.
const STOPPED: u8 = 3;
/// Stopped, and the line given back: counted by the `Arc` alone again.
const DONE: u8 = 4;

/// How a value is counted, kept in it ([`CpuCounted`]).
pub struct CpuCount {
    state: AtomicU8,
    /// Its line, while the state is past [`ARC`].
    line: AtomicU8,
}

impl CpuCount {
    /// Counted by its `Arc` alone, until [`CpuRef::count_on_cpus`].
    pub const fn new() -> CpuCount {
        CpuCount { state: AtomicU8::new(ARC), line: AtomicU8::new(0) }
    }

    fn line(&self) -> usize {
        usize::from(self.line.load(Ordering::Relaxed))
    }

    /// Takes one from the counts of a reference dropped the slow way, and
    /// says whether it was the last of a stopped value: the line is then
    /// back, and the caller drops the `Arc` reference the counts held.
    fn drop_slow(&self) -> bool {
        let _slow = SLOW.lock();
        {
            // In the running CPU's row, held, as `count_on` changes it: no
            // other CPU changes a row while its own may be counting there.
            let held = HeldCpu::here();
            add(count_of(held.index(), self.line()), u32::MAX);
        }
        self.state.load(Ordering::Relaxed) == STOPPED && self.give_back_if_none()
    }

    /// Gives the line back if its counts add up to none, with every count
    /// zero for the next value that takes it. The caller holds the slow
    /// lock, and the value is stopped.
    fn give_back_if_none(&self) -> bool {
        let line = self.line();
        if sum(line) != 0 {
            return false;
        }
        for row in &COUNTS {
            row.0[line].store(0, Ordering::Relaxed);
        }
        self.state.store(DONE, Ordering::Relaxed);
        // The zeros come before the line is free: the next value to take
        // it takes it with an acquire.
        TAKEN.fetch_and(!(1 << line), Ordering::Release);
        true
    }
}

impl Default for CpuCount {
    fn default() -> CpuCount {
        CpuCount::new()
    }
}

/// A value that can be counted on the CPUs: it keeps its [`CpuCount`].
pub trait CpuCounted {
    /// How it is counted.
    fn cpu_count(&self) -> &CpuCount;
}

/// The count of `line` in the row of CPU `cpu`. Both are in range
/// already: the remainders cost an instruction each, where the bounds
/// checks would cost three.
#[inline(always)]
fn count_of(cpu: usize, line: usize) -> &'static AtomicU32 {
    &COUNTS[cpu % CPUS].0[line % LINES]
}

/// Adds `delta` to `count`, a count in the row of the CPU the caller
/// holds. In the kernel only that CPU changes its row, with interrupts
/// off, so a plain load and store do, where an atomic add would lock the
/// line for some twenty cycles (ADR 0406); in the host's builds threads
/// share CPU numbers, and it is an atomic add.
#[inline(always)]
fn add(count: &AtomicU32, delta: u32) {
    if HeldCpu::ALONE {
        count.store(count.load(Ordering::Relaxed).wrapping_add(delta), Ordering::Relaxed);
    } else {
        count.fetch_add(delta, Ordering::Relaxed);
    }
}

/// The count of a reference on CPU `cpu`, the running one, inside an RCU
/// read section with the CPU held: adds `delta` (1, or `u32::MAX` to take
/// one away) to the value's count in that CPU's row, if the value is
/// counted on the CPUs, and says whether it was. The kernel's
/// `radix_count_here` calls it, with interrupts off.
#[inline(always)]
pub fn count_on(cpu: usize, count: &CpuCount, delta: u32) -> bool {
    if count.state.load(Ordering::Acquire) != ON_CPUS {
        return false;
    }
    add(count_of(cpu, count.line()), delta);
    true
}

// The kernel's targets have no operating system beneath them.
#[cfg(target_os = "none")]
#[allow(unsafe_code)]
unsafe extern "Rust" {
    /// [`count_on`] the running CPU, inside a read section: defined by
    /// the kernel (`radix::sched::hooks`), with this signature.
    safe fn radix_count_here(count: &CpuCount, delta: u32) -> bool;
}

/// [`count_on`] the running CPU: the kernel's way (see the module's
/// documentation), or in a read section through the hooks.
#[inline(always)]
fn count_here(count: &CpuCount, delta: u32) -> bool {
    #[cfg(target_os = "none")]
    {
        radix_count_here(count, delta)
    }
    #[cfg(not(target_os = "none"))]
    {
        let read = crate::rcu::read_lock();
        count_on(read.cpu(), count, delta)
    }
}

/// What the counts of `line` add up to. The caller holds the slow lock.
fn sum(line: usize) -> u32 {
    COUNTS.iter().fold(0u32, |sum, row| sum.wrapping_add(row.0[line].load(Ordering::Relaxed)))
}

/// A reference to a value in an `Arc`, counted on the CPUs while the
/// value is (see the module's documentation), and by the `Arc` otherwise.
pub struct CpuRef<T: CpuCounted, A: AllocatorClone> {
    /// The `Arc`. One counted on the CPUs is not counted by it: it is
    /// dropped only in place of the counts' own reference, when the
    /// counts come to none after the value stopped.
    arc: ManuallyDrop<Arc<T, A>>,
    on_cpus: bool,
}

/// How many lines are taken now, for tests.
pub fn lines_taken() -> u32 {
    TAKEN.load(Ordering::Relaxed).count_ones()
}

impl<T: CpuCounted, A: AllocatorClone> CpuRef<T, A> {
    /// A reference counted by `arc`, which it takes.
    pub fn new(arc: Arc<T, A>) -> CpuRef<T, A> {
        CpuRef { arc: ManuallyDrop::new(arc), on_cpus: false }
    }

    /// A copy of `arc` that it does not count.
    #[allow(unsafe_code)]
    fn uncounted(arc: &Arc<T, A>) -> ManuallyDrop<Arc<T, A>> {
        let alloc = Arc::allocator(arc).clone();
        // SAFETY: the pointer is a live `Arc`'s, with its allocator. The
        // copy is never dropped, unless in place of a reference the `Arc`
        // counts: the counts' (see `Drop` and `stop`).
        ManuallyDrop::new(unsafe { Arc::from_raw_in(Arc::as_ptr(arc), alloc) })
    }

    /// Whether `a` and `b` are references to the same value.
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.arc, &b.arc)
    }

    /// Where the value is.
    pub fn as_ptr(this: &Self) -> *const T {
        Arc::as_ptr(&this.arc)
    }

    /// Counts the value on the CPUs from now on, if a line is free;
    /// returns whether it does. Only a value counted by its `Arc` alone,
    /// never on the CPUs before, can be.
    pub fn count_on_cpus(this: &Self) -> bool {
        let count = this.arc.cpu_count();
        if count.state.load(Ordering::Relaxed) != ARC {
            return false;
        }
        // The lowest clear bit, set. (A loop of its own: the update
        // functions of the pinned nightly and of Kani's have other names.)
        let mut taken = TAKEN.load(Ordering::Relaxed);
        loop {
            if taken == u64::MAX {
                return false;
            }
            let with = taken | (taken + 1);
            match TAKEN.compare_exchange_weak(taken, with, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => break,
                Err(now) => taken = now,
            }
        }
        // At most 63, which fits.
        count.line.store((!taken).trailing_zeros() as u8, Ordering::Relaxed);
        // The counts' reference: held until they come to none after
        // `stop`.
        core::mem::forget(Arc::clone(&this.arc));
        // A clone that sees the state sees the line.
        count.state.store(ON_CPUS, Ordering::Release);
        true
    }

    /// Pauses the counting on the CPUs, if the value is counted there:
    /// new references are the `Arc`'s until [`CpuRef::resume`]. Once a
    /// grace period has passed ([`crate::rcu::synchronize`]), no clone or
    /// drop changes a count but under the slow lock, and
    /// [`CpuRef::references`] is exact.
    ///
    /// The owner makes these calls, `pause`, `resume` and `stop`, one at a
    /// time, under a lock of its own.
    pub fn pause(this: &Self) {
        let state = &this.arc.cpu_count().state;
        let _ = state.compare_exchange(ON_CPUS, PAUSED, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Counts the value on the CPUs again, after [`CpuRef::pause`].
    pub fn resume(this: &Self) {
        let state = &this.arc.cpu_count().state;
        let _ = state.compare_exchange(PAUSED, ON_CPUS, Ordering::Release, Ordering::Relaxed);
    }

    /// How many references there are to the value, `this` among them:
    /// exact for a value counted by its `Arc` alone, and for one paused a
    /// grace period ago or more, but for references being made from
    /// others or dropped as it counts, as any count of an `Arc` is.
    pub fn references(this: &Self) -> usize {
        let count = this.arc.cpu_count();
        let _slow = SLOW.lock();
        match count.state.load(Ordering::Relaxed) {
            ARC | DONE => Arc::strong_count(&this.arc),
            // The counts' own reference is the `Arc`'s, not one of these.
            _ => Arc::strong_count(&this.arc) - 1 + sum(count.line()) as usize,
        }
    }

    /// Stops counting the value on the CPUs for good, after
    /// [`CpuRef::pause`] and a grace period. Once the counts come to none,
    /// now or at the last drop of a reference counted on the CPUs, the
    /// line goes back and the counts' `Arc` reference is dropped.
    pub fn stop(this: &Self) {
        let count = this.arc.cpu_count();
        if count.state.load(Ordering::Relaxed) != PAUSED {
            return;
        }
        let last = {
            let _slow = SLOW.lock();
            count.state.store(STOPPED, Ordering::Relaxed);
            count.give_back_if_none()
        };
        if last {
            // `this` is a reference the `Arc` counts, as the counts came
            // to none, so the value outlives the counts' reference.
            drop(ManuallyDrop::into_inner(Self::uncounted(&this.arc)));
        }
    }

    /// Drops a reference counted on the CPUs once its value is not
    /// counted there: under the slow lock, and in place of the counts'
    /// `Arc` reference if it is the last of a stopped value. Out of line,
    /// so that a drop on the CPUs saves no registers for it.
    #[cold]
    #[inline(never)]
    #[allow(unsafe_code)]
    fn drop_slow(&mut self) {
        if self.arc.cpu_count().drop_slow() {
            // SAFETY: the counts came to none after the value stopped, so
            // the counts' reference is dropped, once, in this one's place.
            unsafe { ManuallyDrop::drop(&mut self.arc) };
        }
    }
}

impl<T: CpuCounted, A: AllocatorClone> Clone for CpuRef<T, A> {
    #[inline(always)]
    fn clone(&self) -> Self {
        if count_here(self.arc.cpu_count(), 1) {
            CpuRef { arc: Self::uncounted(&self.arc), on_cpus: true }
        } else {
            CpuRef { arc: ManuallyDrop::new(Arc::clone(&self.arc)), on_cpus: false }
        }
    }
}

impl<T: CpuCounted, A: AllocatorClone> Drop for CpuRef<T, A> {
    #[allow(unsafe_code)]
    #[inline(always)]
    fn drop(&mut self) {
        if !self.on_cpus {
            // SAFETY: a reference the `Arc` counts, dropped once, here.
            unsafe { ManuallyDrop::drop(&mut self.arc) };
            return;
        }
        // A drop on the CPUs needs no order of its own: the owner that
        // adds the counts up waits for its read section to end first, and
        // the last drop of all, which frees the value, comes after it,
        // through the slow lock.
        if !count_here(self.arc.cpu_count(), u32::MAX) {
            self.drop_slow();
        }
    }
}

impl<T: CpuCounted, A: AllocatorClone> Deref for CpuRef<T, A> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.arc
    }
}

#[cfg(test)]
mod tests {
    use std::alloc::Global;
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::rcu;

    /// A value whose drop is seen, and that poisons itself as it goes, so
    /// a reference that outlived it notices.
    struct Value {
        count: CpuCount,
        alive: AtomicBool,
        dropped: &'static AtomicBool,
    }

    impl CpuCounted for Value {
        fn cpu_count(&self) -> &CpuCount {
            &self.count
        }
    }

    impl Drop for Value {
        fn drop(&mut self) {
            self.alive.store(false, Ordering::SeqCst);
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    fn value(dropped: &'static AtomicBool) -> CpuRef<Value, Global> {
        let value = Value { count: CpuCount::new(), alive: AtomicBool::new(true), dropped };
        CpuRef::new(Arc::new_in(value, Global))
    }

    fn line_of(r: &CpuRef<Value, Global>) -> usize {
        r.count.line()
    }

    #[test]
    fn a_value_counted_by_its_arc_alone_counts_as_an_arc() {
        crate::testing::install();
        static DROPPED: AtomicBool = AtomicBool::new(false);
        let a = value(&DROPPED);
        let b = a.clone();
        assert!(CpuRef::ptr_eq(&a, &b));
        assert!(!b.on_cpus);
        assert_eq!(CpuRef::references(&a), 2);
        drop(a);
        assert!(!DROPPED.load(Ordering::SeqCst));
        drop(b);
        assert!(DROPPED.load(Ordering::SeqCst));
    }

    #[test]
    fn a_stopped_value_goes_with_its_last_reference_and_gives_its_line_back() {
        crate::testing::install();
        static DROPPED: AtomicBool = AtomicBool::new(false);
        let a = value(&DROPPED);
        assert!(CpuRef::count_on_cpus(&a));
        assert!(!CpuRef::count_on_cpus(&a), "once only");
        let line = line_of(&a);
        let b = a.clone();
        let c = b.clone();
        assert!(b.on_cpus && c.on_cpus);
        // Dropped on another "CPU" than made it.
        std::thread::spawn(move || drop(c)).join().unwrap();
        CpuRef::pause(&a);
        rcu::synchronize();
        assert_eq!(CpuRef::references(&a), 2);
        // Made while paused: the `Arc`'s.
        let d = b.clone();
        assert!(!d.on_cpus);
        assert_eq!(CpuRef::references(&a), 3);
        drop(d);
        CpuRef::resume(&a);
        assert!(a.clone().on_cpus);
        CpuRef::pause(&a);
        rcu::synchronize();
        CpuRef::stop(&a);
        drop(a);
        assert!(!DROPPED.load(Ordering::SeqCst));
        assert_ne!(TAKEN.load(Ordering::SeqCst) & (1 << line), 0, "b holds the line");
        drop(b);
        assert!(DROPPED.load(Ordering::SeqCst));
        // The line is all zero as it goes back, which MountRefs.tla checks:
        // other tests running now may have taken it again since.
    }

    #[test]
    fn stopping_a_value_with_no_references_on_the_cpus_gives_the_line_back_at_once() {
        crate::testing::install();
        static DROPPED: AtomicBool = AtomicBool::new(false);
        let a = value(&DROPPED);
        assert!(CpuRef::count_on_cpus(&a));
        CpuRef::pause(&a);
        rcu::synchronize();
        assert_eq!(CpuRef::references(&a), 1);
        CpuRef::stop(&a);
        assert_eq!(CpuRef::references(&a), 1);
        drop(a);
        assert!(DROPPED.load(Ordering::SeqCst));
    }

    #[test]
    fn lines_go_back_for_the_next_values() {
        crate::testing::install();
        static DROPPED: AtomicBool = AtomicBool::new(false);
        // Far more values than lines, one after another.
        for _ in 0..4 * LINES {
            let a = value(&DROPPED);
            assert!(CpuRef::count_on_cpus(&a), "a line is free");
            let b = a.clone();
            CpuRef::pause(&a);
            rcu::synchronize();
            CpuRef::stop(&a);
            drop(a);
            drop(b);
        }
    }

    #[test]
    fn references_made_and_dropped_on_many_threads_are_all_seen() {
        crate::testing::install();
        static DROPPED: AtomicBool = AtomicBool::new(false);
        let owner = value(&DROPPED);
        assert!(CpuRef::count_on_cpus(&owner));
        let stop = Arc::new(AtomicBool::new(false));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let (held, stop) = (owner.clone(), Arc::clone(&stop));
                std::thread::spawn(move || {
                    let mut kept = Vec::new();
                    while !stop.load(Ordering::Relaxed) {
                        let r = held.clone();
                        assert!(r.alive.load(Ordering::SeqCst));
                        kept.push(r);
                        if kept.len() > 3 {
                            kept.remove(0);
                        }
                    }
                })
            })
            .collect();
        // Each pause sees the owner, the four threads' own and what they
        // keep, up to four each as one goes in before the oldest goes.
        for _ in 0..50 {
            CpuRef::pause(&owner);
            rcu::synchronize();
            let seen = CpuRef::references(&owner);
            assert!((5..=21).contains(&seen), "{seen} references");
            CpuRef::resume(&owner);
        }
        stop.store(true, Ordering::Relaxed);
        for thread in threads {
            thread.join().unwrap();
        }
        CpuRef::pause(&owner);
        rcu::synchronize();
        assert_eq!(CpuRef::references(&owner), 1, "the threads dropped theirs");
        CpuRef::stop(&owner);
        drop(owner);
        assert!(DROPPED.load(Ordering::SeqCst));
    }
}
