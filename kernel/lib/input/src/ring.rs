// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a reader has not read.
//!
//! Every description of an input file has a ring of its own, so that a
//! reader that stops reading costs no other reader an event, and nothing
//! in the kernel waits for it. A ring that nobody has written to holds no
//! memory. Its first record asks for room for [`Limits::first`], and it
//! doubles when it must, up to [`Limits::most`]. A ring that is read
//! empty after it grew gives its memory back.
//!
//! When what comes does not fit in the most a ring may hold, the reader
//! has lost records and is told once, before the next record it reads:
//! [`Ring::take_lost`]. What it keeps then is its file's choice
//! ([`Full`]): a device's file forgets everything that waited, as half a
//! report or a key's release without its press would mislead; the merged
//! files and the list of devices drop the oldest, as they did before
//! release 1.1.
//!
//! What is pushed in one call stays together: either all of it waits, or
//! none of it.

use alloc::collections::VecDeque;
use core::alloc::Allocator;

/// What a ring keeps when more comes than it may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Full {
    /// Nothing that waited: only what just came.
    Forget,
    /// All but the oldest.
    DropOldest,
}

/// How a ring grows and what it does when it is full.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// How many records it first has room for.
    pub first: usize,
    /// How many it may hold at most.
    pub most: usize,
    /// What it keeps when more comes.
    pub full: Full,
}

/// The records a reader has not read, oldest first.
#[derive(Debug)]
pub struct Ring<T, A: Allocator> {
    records: VecDeque<T, A>,
    /// How many records it has asked the allocator for room for.
    room: usize,
    /// When the newest record it lost came, if the reader has not been
    /// told since.
    lost_ns: Option<u64>,
}

impl<T: Copy, A: Allocator + Copy> Ring<T, A> {
    /// A ring of no record, which holds no memory.
    pub fn new_in(alloc: A) -> Ring<T, A> {
        Ring { records: VecDeque::new_in(alloc), room: 0, lost_ns: None }
    }

    /// How many records wait.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the reader has nothing to read: no record, and no loss to
    /// be told of.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty() && self.lost_ns.is_none()
    }

    /// How many records it holds memory for.
    pub fn room(&self) -> usize {
        self.room
    }

    /// Adds `records`, which came at `time_ns`, as the newest: all of
    /// them or, if they cannot fit even in an empty ring, none.
    pub fn push(&mut self, records: &[T], time_ns: u64, limits: &Limits) {
        let wanted = self.records.len() + records.len();
        if wanted > self.room && !self.grow(wanted, limits) {
            self.lost_ns = Some(time_ns);
            // All the room the ring may have, so that what just came is
            // not lost for want of room it could have had.
            let _ = self.grow(limits.most, limits);
            match limits.full {
                Full::Forget => self.records.clear(),
                Full::DropOldest => {
                    let over = (wanted - self.room).min(self.records.len());
                    self.records.drain(..over);
                }
            }
            if records.len() > self.room {
                return;
            }
        }
        // Within the room reserved: this asks the allocator for nothing.
        self.records.extend(records.iter().copied());
    }

    /// Asks for room for `wanted` records; false if that is more than the
    /// ring may hold or there is no memory.
    fn grow(&mut self, wanted: usize, limits: &Limits) -> bool {
        if wanted > limits.most {
            return false;
        }
        let mut room = self.room.max(limits.first).max(1);
        while room < wanted {
            room = room.saturating_mul(2);
        }
        let room = room.min(limits.most);
        if self.records.try_reserve_exact(room - self.records.len()).is_err() {
            return false;
        }
        self.room = room;
        true
    }

    /// Whether records were lost since the reader was last told, and when
    /// the newest of them came. Asking tells the reader.
    pub fn take_lost(&mut self) -> Option<u64> {
        self.lost_ns.take()
    }

    /// Takes the oldest record. A ring that grew and is now empty gives
    /// its memory back.
    pub fn pop(&mut self, limits: &Limits) -> Option<T> {
        let record = self.records.pop_front()?;
        if self.records.is_empty() && self.room > limits.first {
            self.records = VecDeque::new_in(*self.records.allocator());
            self.room = 0;
        }
        Some(record)
    }

    /// Forgets what waits, and any loss the reader was not told of.
    pub fn clear(&mut self) {
        self.records.clear();
        self.lost_ns = None;
    }
}

#[cfg(test)]
mod tests {
    use alloc::alloc::Global;

    use super::*;

    const DEVICE: Limits = Limits { first: 4, most: 16, full: Full::Forget };
    const MERGED: Limits = Limits { first: 4, most: 8, full: Full::DropOldest };

    fn read(ring: &mut Ring<u32, Global>, limits: &Limits) -> Vec<u32> {
        core::iter::from_fn(|| ring.pop(limits)).collect()
    }

    #[test]
    fn records_are_read_in_the_order_they_came_and_cost_nothing_before() {
        let mut ring: Ring<u32, Global> = Ring::new_in(Global);
        assert!(ring.is_empty());
        assert_eq!(ring.room(), 0);
        assert_eq!(ring.pop(&DEVICE), None);
        ring.push(&[1, 2, 3], 10, &DEVICE);
        assert_eq!((ring.len(), ring.room()), (3, 4));
        assert_eq!(ring.pop(&DEVICE), Some(1));
        ring.push(&[4, 5], 20, &DEVICE);
        assert_eq!(ring.room(), 4);
        assert_eq!(read(&mut ring, &DEVICE), [2, 3, 4, 5]);
        assert_eq!(ring.take_lost(), None);
    }

    #[test]
    fn a_ring_doubles_when_it_must_and_gives_the_memory_back_once_read() {
        let mut ring: Ring<u32, Global> = Ring::new_in(Global);
        for record in 0..5 {
            ring.push(&[record], 0, &DEVICE);
        }
        assert_eq!(ring.room(), 8);
        // One push that needs two doublings gets them.
        let mut big: Ring<u32, Global> = Ring::new_in(Global);
        big.push(&[7; 13], 0, &DEVICE);
        assert_eq!(big.room(), 16);
        assert_eq!(read(&mut ring, &DEVICE), [0, 1, 2, 3, 4]);
        assert_eq!(ring.room(), 0);
        assert_eq!(ring.take_lost(), None);
        // A ring that never grew keeps its first room.
        ring.push(&[9], 0, &DEVICE);
        assert_eq!(ring.pop(&DEVICE), Some(9));
        assert_eq!(ring.room(), 4);
    }

    #[test]
    fn a_device_s_ring_that_is_full_forgets_what_waited_and_says_so_once() {
        let mut ring: Ring<u32, Global> = Ring::new_in(Global);
        for record in 0..15 {
            ring.push(&[record], u64::from(record), &DEVICE);
        }
        // What comes stays together: two do not fit beside fifteen.
        ring.push(&[100, 101], 99, &DEVICE);
        assert!(!ring.is_empty());
        assert_eq!(ring.take_lost(), Some(99));
        assert_eq!(ring.take_lost(), None);
        assert_eq!(read(&mut ring, &DEVICE), [100, 101]);
    }

    #[test]
    fn what_overflows_a_small_ring_still_gets_the_most_room() {
        let mut ring: Ring<u32, Global> = Ring::new_in(Global);
        ring.push(&[1, 2, 3, 4], 1, &DEVICE);
        // Eighteen do not fit in sixteen, and fourteen do.
        ring.push(&[5; 14], 2, &DEVICE);
        assert_eq!(ring.take_lost(), Some(2));
        assert_eq!((ring.len(), ring.room()), (14, 16));
    }

    #[test]
    fn a_merged_ring_that_is_full_drops_the_oldest() {
        let mut ring: Ring<u32, Global> = Ring::new_in(Global);
        for record in 0..8 {
            ring.push(&[record], u64::from(record), &MERGED);
        }
        assert_eq!(ring.take_lost(), None);
        ring.push(&[8, 9, 10], 50, &MERGED);
        assert_eq!(ring.take_lost(), Some(50));
        assert_eq!(read(&mut ring, &MERGED), [3, 4, 5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn more_than_a_ring_may_hold_at_once_is_lost_whole() {
        let mut ring: Ring<u32, Global> = Ring::new_in(Global);
        ring.push(&[1], 1, &DEVICE);
        ring.push(&[0; 17], 2, &DEVICE);
        assert_eq!(ring.take_lost(), Some(2));
        assert_eq!(ring.len(), 0);
        // Told of the loss, the reader has nothing more to read.
        assert!(ring.is_empty());
    }

    #[test]
    fn clearing_forgets_the_records_and_the_loss() {
        let mut ring: Ring<u32, Global> = Ring::new_in(Global);
        ring.push(&[0; 17], 2, &DEVICE);
        ring.push(&[1, 2], 3, &DEVICE);
        ring.clear();
        assert!(ring.is_empty());
        assert_eq!(ring.take_lost(), None);
    }
}
