// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Full barriers, as the kernel's synchronization takes them: a waker's,
//! between the change it made and its look for waiters, and a waiter's,
//! between counting itself and its look at the change (ADR 0773).
//!
//! `fence(Ordering::SeqCst)` is an MFENCE on x86-64, which orders
//! non-temporal stores and cache-line flushes as well as ordinary memory,
//! and takes about twice the cycles of a locked instruction, which orders
//! ordinary memory as fully. [`full`] is a locked instruction there, as
//! Linux's `smp_mb` is; code that orders device memory, write-combining
//! stores or flushes keeps the fence. Kani's model checks the fence.

use core::sync::atomic::Ordering;

/// A full barrier for ordinary memory: no load or store after it is done
/// before a load or store before it, as `fence(Ordering::SeqCst)`.
#[inline(always)]
pub fn full() {
    #[cfg(all(target_arch = "x86_64", not(kani)))]
    // SAFETY: a locked OR of zero into the word at the stack pointer
    // changes no value, and nothing else writes that word meanwhile: it
    // is this thread's stack. The block may read and write memory, so the
    // compiler moves no access across it either.
    unsafe {
        core::arch::asm!("lock or dword ptr [rsp], 0", options(nostack));
    }
    #[cfg(not(all(target_arch = "x86_64", not(kani))))]
    core::sync::atomic::fence(Ordering::SeqCst);
}

/// The full barrier after a sequentially consistent read-modify-write,
/// for the loads after it: none on x86-64, whose locked instructions are
/// full barriers already (Linux's `smp_mb__after_atomic`), but for the
/// compiler's; a fence elsewhere.
#[inline(always)]
pub fn after_rmw() {
    #[cfg(all(target_arch = "x86_64", not(kani)))]
    core::sync::atomic::compiler_fence(Ordering::SeqCst);
    #[cfg(not(all(target_arch = "x86_64", not(kani))))]
    core::sync::atomic::fence(Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;

    /// Store buffering, the pattern the barriers are for: each thread
    /// stores its flag, takes the barrier, then loads the other's. Without
    /// a full barrier both loads may see the old values; with one, at
    /// least one thread sees the other's store.
    fn store_buffering(barrier: fn(), rmw: bool) {
        for _ in 0..20_000 {
            let flags = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
            let seen = Arc::new([AtomicBool::new(false), AtomicBool::new(false)]);
            let go = Arc::new(AtomicUsize::new(0));
            let threads: std::vec::Vec<_> = (0..2)
                .map(|me| {
                    let (flags, seen, go) = (flags.clone(), seen.clone(), go.clone());
                    thread::spawn(move || {
                        go.fetch_add(1, Ordering::SeqCst);
                        while go.load(Ordering::Relaxed) < 2 {
                            core::hint::spin_loop();
                        }
                        if rmw {
                            flags[me].fetch_add(1, Ordering::SeqCst);
                        } else {
                            flags[me].store(1, Ordering::Relaxed);
                        }
                        barrier();
                        let other = flags[1 - me].load(Ordering::Relaxed);
                        seen[me].store(other != 0, Ordering::Relaxed);
                    })
                })
                .collect();
            for thread in threads {
                thread.join().unwrap();
            }
            assert!(
                seen[0].load(Ordering::Relaxed) || seen[1].load(Ordering::Relaxed),
                "both threads missed the other's store"
            );
        }
    }

    #[test]
    fn a_full_barrier_keeps_a_store_from_passing_the_load_after_it() {
        store_buffering(super::full, false);
    }

    #[test]
    fn a_read_modify_write_and_its_barrier_do_too() {
        store_buffering(super::after_rmw, true);
    }
}
