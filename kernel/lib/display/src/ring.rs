// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The events a holder has not read.
//!
//! A holder is told of beats, screens and the front with DisplayEvent
//! structures it reads from its file. Those it has not read wait in a
//! ring, oldest first. A holder that stops reading must not cost the
//! kernel memory without end, nor hold up the card's thread that writes
//! them: the ring holds as many as its storage, and when one more comes
//! the oldest is dropped. The holder then reads an EVENT_OVERFLOW before
//! any other, and knows to ask the layer for its state again.
//!
//! The storage is the kernel's: a slice the ring is given at each call,
//! the same one each time, so that a holder that reads its events costs
//! a few of them and only one that falls behind is given EVENTS_KEPT.
//! [`Ring::move_to`] carries what waits into a longer slice.

use radix_abi::display::{DisplayEvent, EVENT_OVERFLOW};

/// Where a holder's unread events are in their storage.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ring {
    /// The place of the oldest.
    head: usize,
    /// How many wait.
    len: usize,
    /// The time of the newest event that was dropped, if any was since
    /// the holder last read.
    lost_ns: Option<u64>,
}

impl Ring {
    /// A ring of no event.
    pub const fn new() -> Ring {
        Ring { head: 0, len: 0, lost_ns: None }
    }

    /// How many events there are to read, the mark of those lost among
    /// them.
    pub fn waiting(&self) -> usize {
        self.len + usize::from(self.lost_ns.is_some())
    }

    /// Whether there is nothing to read.
    pub fn is_empty(&self) -> bool {
        self.waiting() == 0
    }

    /// Whether `storage` holds as many as it can, so that the next event
    /// drops the oldest unless the storage grows first.
    pub fn is_full(&self, storage: &[DisplayEvent]) -> bool {
        self.len >= storage.len()
    }

    /// Adds `event` as the newest. When `storage` is full the oldest is
    /// dropped and marked lost.
    pub fn push(&mut self, storage: &mut [DisplayEvent], event: DisplayEvent) {
        let Some(place) = self.place(storage.len(), self.len) else {
            // No storage at all: the event itself is the one lost.
            self.lost_ns = Some(event.time_ns);
            return;
        };
        if self.len == storage.len() {
            self.lost_ns = Some(storage[self.head].time_ns);
            self.head = (self.head + 1) % storage.len();
        } else {
            self.len += 1;
        }
        storage[place] = event;
    }

    /// Takes the oldest event: an EVENT_OVERFLOW first if any was lost,
    /// with the time of the newest of them.
    pub fn pop(&mut self, storage: &[DisplayEvent]) -> Option<DisplayEvent> {
        if let Some(time_ns) = self.lost_ns.take() {
            return Some(DisplayEvent { kind: EVENT_OVERFLOW, time_ns, ..DisplayEvent::default() });
        }
        if self.len == 0 {
            return None;
        }
        let event = *storage.get(self.head)?;
        self.head = (self.head + 1) % storage.len();
        self.len -= 1;
        Some(event)
    }

    /// Carries what waits from `old` into `new`, which must be at least
    /// as long, and which is the storage from then on. Returns false,
    /// and changes nothing, if `new` is shorter than what waits.
    pub fn move_to(&mut self, old: &[DisplayEvent], new: &mut [DisplayEvent]) -> bool {
        if new.len() < self.len || old.len() < self.len {
            return false;
        }
        for (index, slot) in new.iter_mut().enumerate().take(self.len) {
            *slot = old[(self.head + index) % old.len()];
        }
        self.head = 0;
        true
    }

    /// Where the event `index` places after the oldest is, or goes, in
    /// storage of `capacity`; `None` if there is no storage.
    fn place(&self, capacity: usize, index: usize) -> Option<usize> {
        (capacity != 0).then(|| (self.head % capacity + index % capacity) % capacity)
    }
}

#[cfg(test)]
mod tests {
    use radix_abi::display::{EVENT_BEAT, EVENT_FRONT};

    use super::*;

    fn beat(count: u64) -> DisplayEvent {
        DisplayEvent { kind: EVENT_BEAT, count, time_ns: count * 10, ..DisplayEvent::default() }
    }

    fn counts(ring: &mut Ring, storage: &[DisplayEvent]) -> Vec<(u32, u64)> {
        core::iter::from_fn(|| ring.pop(storage)).map(|event| (event.kind, event.count)).collect()
    }

    #[test]
    fn events_are_read_in_the_order_they_came() {
        let mut storage = [DisplayEvent::default(); 4];
        let mut ring = Ring::new();
        assert!(ring.is_empty());
        assert_eq!(ring.pop(&storage), None);
        for count in 1..=3 {
            ring.push(&mut storage, beat(count));
        }
        assert_eq!(ring.waiting(), 3);
        assert!(!ring.is_full(&storage));
        assert_eq!(ring.pop(&storage), Some(beat(1)));
        // The ring goes round the end of its storage.
        for count in 4..=5 {
            ring.push(&mut storage, beat(count));
        }
        assert!(ring.is_full(&storage));
        let read = counts(&mut ring, &storage);
        assert_eq!(read, [2, 3, 4, 5].map(|count| (EVENT_BEAT, count)));
        assert!(ring.is_empty());
    }

    #[test]
    fn a_full_ring_drops_the_oldest_and_says_so_first() {
        let mut storage = [DisplayEvent::default(); 4];
        let mut ring = Ring::new();
        for count in 1..=7 {
            ring.push(&mut storage, beat(count));
        }
        // Four wait, and the mark.
        assert_eq!(ring.waiting(), 5);
        let lost = ring.pop(&storage).unwrap();
        assert_eq!((lost.kind, lost.time_ns), (EVENT_OVERFLOW, 30));
        assert_eq!(ring.waiting(), 4);
        let read = counts(&mut ring, &storage);
        assert_eq!(read, [4, 5, 6, 7].map(|count| (EVENT_BEAT, count)));
        // The mark is read once.
        ring.push(&mut storage, beat(8));
        assert_eq!(ring.pop(&storage), Some(beat(8)));
        assert_eq!(ring.pop(&storage), None);
    }

    #[test]
    fn what_waits_moves_into_longer_storage_in_order() {
        let mut small = [DisplayEvent::default(); 4];
        let mut ring = Ring::new();
        for count in 1..=6 {
            ring.push(&mut small, beat(count));
        }
        let mut large = [DisplayEvent::default(); 8];
        assert!(ring.move_to(&small, &mut large));
        assert!(!ring.is_full(&large));
        for count in 7..=10 {
            ring.push(&mut large, beat(count));
        }
        // The two lost before the move are still marked.
        let read = counts(&mut ring, &large);
        assert_eq!(read[0], (EVENT_OVERFLOW, 0));
        assert_eq!(read[1..], [3, 4, 5, 6, 7, 8, 9, 10].map(|count| (EVENT_BEAT, count)));
    }

    #[test]
    fn storage_too_short_for_what_waits_is_not_moved_into() {
        let mut storage = [DisplayEvent::default(); 4];
        let mut ring = Ring::new();
        for count in 1..=3 {
            ring.push(&mut storage, beat(count));
        }
        let mut short = [DisplayEvent::default(); 2];
        assert!(!ring.move_to(&storage, &mut short));
        assert_eq!(ring.pop(&storage), Some(beat(1)));
    }

    #[test]
    fn a_ring_with_no_storage_loses_every_event_and_says_so() {
        let mut ring = Ring::new();
        let front = DisplayEvent { kind: EVENT_FRONT, time_ns: 77, ..DisplayEvent::default() };
        ring.push(&mut [], front);
        assert_eq!(ring.waiting(), 1);
        let lost = ring.pop(&[]).unwrap();
        assert_eq!((lost.kind, lost.time_ns), (EVENT_OVERFLOW, 77));
        assert_eq!(ring.pop(&[]), None);
        // It then moves into its first storage.
        let mut storage = [DisplayEvent::default(); 2];
        assert!(ring.move_to(&[], &mut storage));
        ring.push(&mut storage, front);
        assert_eq!(ring.pop(&storage), Some(front));
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// Whatever a ring is asked, with storage of any length it may have
    /// been given, it reads and writes inside the storage, and holds no
    /// more than the storage does.
    #[kani::proof]
    #[kani::unwind(6)]
    fn a_ring_stays_inside_its_storage() {
        let mut storage = [DisplayEvent::default(); 4];
        let capacity: usize = kani::any_where(|&capacity: &usize| capacity <= 4);
        let mut ring = Ring::new();
        for _ in 0..5 {
            if kani::any() {
                let event = DisplayEvent { count: kani::any(), ..DisplayEvent::default() };
                ring.push(&mut storage[..capacity], event);
            } else {
                let _ = ring.pop(&storage[..capacity]);
            }
            assert!(ring.len <= capacity);
            assert!(ring.head < capacity || ring.head == 0);
        }
    }
}
