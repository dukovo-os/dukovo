// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A CPU's request to switch threads (`need_resched`), and the threads
//! other CPUs woke for it to queue (its inbox), in one word (ADR 0555,
//! ADR 0556).
//!
//! An idle CPU spins on this word while it polls, and a CPU in MWAIT
//! watches its line, so the kernel keeps the line for what a waker needs:
//! this word, and the load the CPU publishes, which a waker reads to
//! choose the CPU just before it writes the word. Whatever else the line
//! held, the two CPUs would pass it back and forth with each write, and on
//! a large chip each pass takes some hundreds of nanoseconds. A wake-up
//! from another CPU is therefore one atomic read-modify-write: it asks for
//! the switch, marks the request as from another CPU, and learns from the
//! same word whether the idle CPU watches it or must be sent an interrupt.
//!
//! A thread woken for an idle CPU goes into the same word: the word's
//! upper bits hold the address of the newest such thread, and each one
//! the address of the one before it. The waker then writes nothing of the
//! idle CPU's but this line, and the idle CPU queues the threads on its
//! own run queue, whose lines stay in its own cache. The kernel links the
//! entries; this type only keeps the newest one's address.

use core::sync::atomic::{AtomicUsize, Ordering};

/// A switch is asked for. The lowest bit, which `Cpu::idle_until` checks.
const PENDING: usize = 1 << 0;
/// The CPU's idle thread watches the word, polling it or in MWAIT, so a
/// waker need send no interrupt.
const WATCHED: usize = 1 << 1;
/// Asked from another CPU since the CPU last went idle, which tells
/// [`crate::HaltPoll`] which idle periods polling could have caught.
const REMOTE: usize = 1 << 2;
/// The bits below an inbox entry's address.
const FLAGS: usize = PENDING | WATCHED | REMOTE;

/// The alignment an inbox entry's address needs, which leaves the flags'
/// bits free below it.
pub const ENTRY_ALIGN: usize = FLAGS + 1;

/// One CPU's request to switch threads, and its inbox. The kernel gives
/// it a cache line, shared only with what a waker reads to choose the CPU.
pub struct Resched(AtomicUsize);

impl Resched {
    /// No switch asked for, not watched, and an empty inbox.
    pub const fn new() -> Self {
        Self(AtomicUsize::new(0))
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

    /// Puts `entry`, an address aligned to [`ENTRY_ALIGN`], in the inbox
    /// of the CPU that owns the word, from another CPU, and asks that CPU
    /// to switch, in one step. `link` stores in the entry the address of
    /// the entry before it (0 for none); it runs before the entry is
    /// published, and again each time another CPU's entry gets in first.
    /// Returns whether that CPU watches the word; if not, the caller must
    /// send it an interrupt.
    #[inline]
    pub fn hand_over(&self, entry: usize, mut link: impl FnMut(usize)) -> bool {
        debug_assert!(entry != 0 && entry & FLAGS == 0, "an inbox entry must be aligned");
        // Guessed as an idle CPU that watches the word holds it, with
        // nothing in it: reading it first would bring the line here only
        // to bring it here again, to own, for the exchange. A wrong guess
        // costs a second exchange on a line already here.
        let mut seen = WATCHED;
        loop {
            link(seen & !FLAGS);
            let handed = entry | (seen & FLAGS) | PENDING | REMOTE;
            match self.0.compare_exchange_weak(seen, handed, Ordering::SeqCst, Ordering::Relaxed) {
                Ok(_) => return seen & WATCHED != 0,
                Err(now) => seen = now,
            }
        }
    }

    /// Withdraws the request, as the switch it asked for begins, and
    /// empties the inbox. Returns `None` if no switch was asked for, else
    /// the address of the inbox's newest entry, or 0 if it was empty. Only
    /// the owning CPU calls it, never while its idle thread watches the
    /// word. A request that comes after it stays pending, for the switch
    /// after this one.
    #[inline]
    pub fn take(&self) -> Option<usize> {
        // Most switches find nothing asked for: a load of the CPU's own
        // line, rather than a locked exchange.
        if self.0.load(Ordering::Relaxed) == 0 {
            return None;
        }
        let word = self.0.swap(0, Ordering::Acquire);
        debug_assert_eq!(word & WATCHED, 0, "taken while the idle thread watched");
        (word & PENDING != 0).then_some(word & !FLAGS)
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
    pub fn word(&self) -> &AtomicUsize {
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
    fn a_request_from_this_cpu_is_pending_until_taken() {
        let word = Resched::new();
        assert!(!word.pending());
        assert_eq!(word.take(), None, "nothing asked for");
        word.request();
        assert!(word.pending());
        assert!(!word.remote());
        assert_eq!(word.take(), Some(0));
        assert!(!word.pending());
    }

    #[test]
    fn a_remote_request_needs_an_interrupt_unless_watched() {
        let word = Resched::new();
        assert!(!word.request_remote(), "unwatched: interrupt");
        assert!(word.pending() && word.remote());
        word.take();
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
        word.hand_over(0x1000, |_| {});
        assert_eq!(word.word().load(Ordering::Relaxed) & 1, 1);
    }

    #[test]
    fn handing_over_asks_for_the_switch_and_says_whether_watched() {
        let word = Resched::new();
        assert!(!word.hand_over(0x1000, |_| {}), "unwatched: interrupt");
        assert!(word.pending() && word.remote());
        assert_eq!(word.take(), Some(0x1000));
        assert!(!word.pending());
        word.watch();
        assert!(word.hand_over(0x2000, |_| {}), "watched: no interrupt");
        assert!(word.unwatch());
        assert_eq!(word.take(), Some(0x2000));
    }

    #[test]
    fn each_entry_links_to_the_one_before_it() {
        let word = Resched::new();
        let mut links = Vec::new();
        for entry in [0x1000, 0x2000, 0x3000] {
            let mut before = None;
            word.hand_over(entry, |previous| before = Some(previous));
            links.push(before.unwrap());
        }
        assert_eq!(links, [0, 0x1000, 0x2000]);
        assert_eq!(word.take(), Some(0x3000));
        assert_eq!(word.take(), None, "taking empties the inbox");
    }

    #[test]
    fn a_local_request_keeps_the_inbox() {
        let word = Resched::new();
        word.hand_over(0x1000, |_| {});
        word.request();
        assert!(word.go_idle());
        assert_eq!(word.take(), Some(0x1000));
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

    /// Wakers on several threads hand over entries while the owner takes
    /// the inbox now and then; every entry is taken exactly once, and each
    /// taken list holds its entries newest first.
    #[test]
    fn every_entry_handed_over_is_taken_once() {
        const WAKERS: usize = 4;
        const EACH: usize = 500;
        let word = Arc::new(Resched::new());
        // Entry `e`'s link lives at links[e / ENTRY_ALIGN - 1].
        let links: Arc<Vec<AtomicUsize>> =
            Arc::new((0..WAKERS * EACH).map(|_| AtomicUsize::new(0)).collect());
        let wakers: Vec<_> = (0..WAKERS)
            .map(|w| {
                let (word, links) = (Arc::clone(&word), Arc::clone(&links));
                std::thread::spawn(move || {
                    for i in 0..EACH {
                        let index = w * EACH + i;
                        let entry = (index + 1) * ENTRY_ALIGN;
                        word.hand_over(entry, |before| {
                            links[index].store(before, Ordering::Relaxed);
                        });
                    }
                })
            })
            .collect();
        let mut taken = vec![false; WAKERS * EACH];
        let mut count = 0;
        while count < WAKERS * EACH {
            let mut entry = word.take().unwrap_or(0);
            while entry != 0 {
                let index = entry / ENTRY_ALIGN - 1;
                assert!(!taken[index], "entry {entry:#x} taken twice");
                taken[index] = true;
                count += 1;
                entry = links[index].load(Ordering::Relaxed);
            }
        }
        for waker in wakers {
            waker.join().unwrap();
        }
        assert_eq!(word.take(), None);
    }
}
