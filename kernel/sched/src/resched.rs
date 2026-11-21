// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A CPU's request to switch threads (`need_resched`), in one word alone
//! in its cache line (ADR 0555).
//!
//! An idle CPU spins on this word while it polls, and a CPU in MWAIT
//! watches its line, so a waker's write should be the only traffic the
//! line sees: whatever else the line held, the two CPUs would pass it
//! back and forth with each write, and on a large chip each pass takes
//! some hundreds of nanoseconds. A wake-up from another CPU is therefore
//! one atomic read-modify-write: it asks for the switch, marks the
//! request as from another CPU, and learns from the same word whether the
//! idle CPU watches it or must be sent an interrupt.

use core::sync::atomic::{AtomicU32, Ordering};

/// A switch is asked for. The lowest bit, which `Cpu::idle_until` checks.
const PENDING: u32 = 1 << 0;
/// The CPU's idle thread watches the word, polling it or in MWAIT, so a
/// waker need send no interrupt.
const WATCHED: u32 = 1 << 1;
/// Asked from another CPU since the CPU last went idle, which tells
/// [`crate::HaltPoll`] which idle periods polling could have caught.
const REMOTE: u32 = 1 << 2;

/// One CPU's request to switch threads.
#[repr(align(64))]
pub struct Resched(AtomicU32);

impl Resched {
    /// No switch asked for, and not watched.
    pub const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// Whether a switch is asked for.
    #[inline]
    pub fn pending(&self) -> bool {
        self.0.load(Ordering::Relaxed) & PENDING != 0
    }

    /// Asks the CPU that owns the word, from that CPU, to switch: when it
    /// next lets go of its last spin lock, or leaves the interrupt it is in.
    #[inline]
    pub fn request(&self) {
        self.0.fetch_or(PENDING, Ordering::SeqCst);
    }

    /// Asks the CPU that owns the word, from another CPU, to switch.
    /// Returns whether that CPU watches the word; if not, the caller must
    /// send it an interrupt.
    #[inline]
    pub fn request_remote(&self) -> bool {
        self.0.fetch_or(PENDING | REMOTE, Ordering::SeqCst) & WATCHED != 0
    }

    /// Withdraws the request, as the switch it asked for begins. Only the
    /// owning CPU calls it, never while its idle thread watches the word.
    /// A request from another CPU that this overwrites is for a thread
    /// already on the run queue, which the switch picks from.
    #[inline]
    pub fn clear(&self) {
        debug_assert_eq!(self.0.load(Ordering::Relaxed) & WATCHED, 0);
        self.0.store(0, Ordering::Relaxed);
    }

    /// The idle thread starting an idle period: forgets whether the last
    /// request came from another CPU. Returns whether a switch is asked for.
    #[inline]
    pub fn go_idle(&self) -> bool {
        self.0.fetch_and(!REMOTE, Ordering::SeqCst) & PENDING != 0
    }

    /// The idle thread starting to watch the word. Wakers stop sending
    /// interrupts. Returns whether a switch is asked for already.
    #[inline]
    pub fn watch(&self) -> bool {
        self.0.fetch_or(WATCHED, Ordering::SeqCst) & PENDING != 0
    }

    /// The idle thread no longer watching the word. Returns whether a
    /// switch is asked for. Either a waker's request came first, and this
    /// sees it, or it comes after, sees the word unwatched and sends the
    /// interrupt: the word orders the two.
    #[inline]
    pub fn unwatch(&self) -> bool {
        self.0.fetch_and(!WATCHED, Ordering::SeqCst) & PENDING != 0
    }

    /// Whether a request from another CPU came since [`Resched::go_idle`].
    #[inline]
    pub fn remote(&self) -> bool {
        self.0.load(Ordering::Relaxed) & REMOTE != 0
    }

    /// The word, for `Cpu::idle_until`: a write to its line ends MWAIT,
    /// and its lowest bit is set while a switch is asked for.
    pub fn word(&self) -> &AtomicU32 {
        &self.0
    }
}

impl Default for Resched {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn the_word_has_its_cache_line_to_itself() {
        assert_eq!(core::mem::align_of::<Resched>(), 64);
        assert_eq!(core::mem::size_of::<Resched>(), 64);
    }

    #[test]
    fn a_request_from_this_cpu_is_pending_until_cleared() {
        let word = Resched::new();
        assert!(!word.pending());
        word.request();
        assert!(word.pending());
        assert!(!word.remote());
        word.clear();
        assert!(!word.pending());
    }

    #[test]
    fn a_remote_request_needs_an_interrupt_unless_watched() {
        let word = Resched::new();
        assert!(!word.request_remote(), "unwatched: interrupt");
        assert!(word.pending() && word.remote());
        word.clear();
        assert!(!word.watch());
        assert!(word.request_remote(), "watched: no interrupt");
        assert!(word.unwatch());
    }

    #[test]
    fn going_idle_forgets_the_remote_mark_but_not_the_request() {
        let word = Resched::new();
        word.request_remote();
        assert!(word.go_idle());
        assert!(word.pending());
        assert!(!word.remote());
    }

    #[test]
    fn the_word_holds_its_lowest_bit_while_pending() {
        let word = Resched::new();
        word.watch();
        assert_eq!(word.word().load(Ordering::Relaxed) & 1, 0);
        word.request_remote();
        assert_eq!(word.word().load(Ordering::Relaxed) & 1, 1);
    }

    /// The idle side watches, polls a while, then stops watching and would
    /// halt if nothing is pending; the waker requests and would interrupt
    /// if the word was unwatched. A wake-up is never lost: the idle side
    /// sees the request, or the waker sends the interrupt.
    #[test]
    fn a_wake_up_is_never_lost() {
        for _ in 0..2000 {
            let word = Arc::new(Resched::new());
            let interrupted = Arc::new(AtomicBool::new(false));
            let waker = {
                let (word, interrupted) = (Arc::clone(&word), Arc::clone(&interrupted));
                std::thread::spawn(move || {
                    if !word.request_remote() {
                        interrupted.store(true, Ordering::SeqCst);
                    }
                })
            };
            let mut seen = word.watch();
            for _ in 0..50 {
                seen |= word.pending();
                std::hint::spin_loop();
            }
            seen |= word.unwatch();
            waker.join().unwrap();
            assert!(seen || interrupted.load(Ordering::SeqCst));
        }
    }
}
