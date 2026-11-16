// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The memory test a start runs once before the first program, when the
//! command line has `memtest=once` (hardware(8), ADR 0565).
//!
//! The kernel takes the frames that are free, hands them here as blocks,
//! and gives them back after. Each step runs across every block before
//! the next, so that what is read comes from the memory and not from the
//! caches, which hold far less than the blocks:
//!
//! 1. each word is written its own physical address;
//! 2. read and checked, then 0x55 in every byte is written;
//! 3. read and checked, then the inverse, 0xAA, is written;
//! 4. read and checked, then 0x55 again;
//! 5. read and checked, then a random word made from a seed and the
//!    word's address;
//! 6. read and checked.
//!
//! So the address pattern finds two addresses that reach one word, and
//! the moving inversions a bit that holds one value or cannot hold one.
//! Each page is read twice, and a word that either read gets wrong is an
//! error, so that a read that fails now and then counts too. The frames
//! with errors are kept for the kernel, which takes them out of use.

use crate::page::PAGE_SIZE;

/// The words of a page.
pub const PAGE_WORDS: usize = PAGE_SIZE / 8;

/// The most frames with errors to keep room for: as many as the frame
/// allocator retires. Errors in more are counted but not placed.
pub const BAD_MAX: usize = crate::frames::MAX_RETIRED;

/// The most ranges of bad frames said.
pub const RANGES_MAX: usize = 64;

/// The bytes of scratch a test needs: three pages.
pub const SCRATCH: usize = 3 * PAGE_SIZE;

/// What is written to each word in a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fill {
    /// The word's own physical address.
    Address,
    /// The same word everywhere.
    Constant(u64),
    /// A random word made from the seed and the word's address.
    Random,
}

const FIVES: u64 = 0x5555_5555_5555_5555;
const TENS: u64 = 0xaaaa_aaaa_aaaa_aaaa;

/// The steps: what each checks, and what it writes after.
const STEPS: [(Option<Fill>, Option<Fill>); 6] = [
    (None, Some(Fill::Address)),
    (Some(Fill::Address), Some(Fill::Constant(FIVES))),
    (Some(Fill::Constant(FIVES)), Some(Fill::Constant(TENS))),
    (Some(Fill::Constant(TENS)), Some(Fill::Constant(FIVES))),
    (Some(Fill::Constant(FIVES)), Some(Fill::Random)),
    (Some(Fill::Random), None),
];

/// How many steps a test makes over every page.
pub const STEP_COUNT: u64 = STEPS.len() as u64;

/// The memory under test, a page at a time, by physical address.
pub trait Ram {
    /// Writes `bytes` to the page at `at`.
    fn write(&mut self, at: u64, bytes: &[u8; PAGE_SIZE]);
    /// Reads the page at `at` into `bytes`.
    fn read(&mut self, at: u64, bytes: &mut [u8; PAGE_SIZE]);
}

/// A word made from `seed` and `address`: SplitMix64's mix, so that each
/// word differs from its neighbours and the seed changes them all.
fn random(seed: u64, address: u64) -> u64 {
    let mut z = seed ^ address.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The word `fill` puts at `address`.
fn value(fill: Fill, seed: u64, address: u64) -> u64 {
    match fill {
        Fill::Address => address,
        Fill::Constant(word) => word,
        Fill::Random => random(seed, address),
    }
}

/// The frames found bad, by their physical address, each with its count
/// of errors, in order, in room the caller gives.
#[derive(Debug)]
pub struct Bad<'a> {
    frames: &'a mut [(u64, u32)],
    len: usize,
}

impl<'a> Bad<'a> {
    /// No frames yet, with `room` for as many as it holds.
    pub fn new(room: &'a mut [(u64, u32)]) -> Bad<'a> {
        Bad { frames: room, len: 0 }
    }

    /// Counts `errors` in the frame at `frame`; false if it is a new frame
    /// and the room is full.
    fn add(&mut self, frame: u64, errors: u32) -> bool {
        let kept = &self.frames[..self.len];
        match kept.binary_search_by_key(&frame, |&(at, _)| at) {
            Ok(at) => {
                self.frames[at].1 = self.frames[at].1.saturating_add(errors);
                true
            }
            Err(_) if self.len == self.frames.len() => false,
            Err(at) => {
                self.frames.copy_within(at..self.len, at + 1);
                self.frames[at] = (frame, errors);
                self.len += 1;
                true
            }
        }
    }

    /// The frames kept, by physical address, with their errors.
    pub fn frames(&self) -> &[(u64, u32)] {
        &self.frames[..self.len]
    }

    /// The frames kept, those next to each other as one range: its first
    /// byte, its last byte and its errors; at most [`RANGES_MAX`].
    pub fn ranges(&self) -> impl Iterator<Item = (u64, u64, u64)> + '_ {
        let size = PAGE_SIZE as u64;
        let mut at = 0;
        core::iter::from_fn(move || {
            let &(first, errors) = self.frames().get(at)?;
            let (mut last, mut count) = (first, u64::from(errors));
            at += 1;
            while let Some(&(next, errors)) = self.frames().get(at)
                && next == last + size
            {
                (last, count) = (next, count + u64::from(errors));
                at += 1;
            }
            Some((first, last + size - 1, count))
        })
        .take(RANGES_MAX)
    }
}

/// What a test is run over, and the memory it works in.
pub struct Test<'a> {
    /// The blocks: each its first physical address and its pages.
    pub blocks: &'a [(u64, u64)],
    /// The seed of the random step.
    pub seed: u64,
    /// One bit of the first word of the first page is flipped after the
    /// first step, as memory that went bad would, so that a machine
    /// whose memory never fails shows the errors' path.
    pub fault: bool,
    /// Three pages to read and write through.
    pub scratch: &'a mut [u8; SCRATCH],
    /// Room for the bad frames, [`BAD_MAX`] of them in the kernel.
    pub room: &'a mut [(u64, u32)],
}

/// How a test ended.
#[derive(Debug)]
pub struct Outcome<'a> {
    /// Whether every step ran over every page, before the time ran out.
    pub complete: bool,
    /// The words found wrong, a word once in each step.
    pub errors: u64,
    /// The frames found bad.
    pub bad: Bad<'a>,
    /// Errors in frames past the room, which are not in `bad`.
    pub unplaced: u64,
}

/// The word at `word` of `page`.
fn word(page: &[u8; PAGE_SIZE], word: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&page[8 * word..8 * word + 8]);
    u64::from_le_bytes(bytes)
}

/// Runs `test` on `ram`. `out_of_time` is asked after each block and
/// ends the test early when it says so; `progress` is told after each
/// block how many pages of all the steps are done, of how many.
pub fn run<'a>(
    ram: &mut impl Ram,
    test: Test<'a>,
    mut out_of_time: impl FnMut() -> bool,
    mut progress: impl FnMut(u64, u64),
) -> Outcome<'a> {
    let Test { blocks, seed, fault, scratch, room } = test;
    let (written, rest) = scratch.split_at_mut(PAGE_SIZE);
    let (first, second) = rest.split_at_mut(PAGE_SIZE);
    let (Ok(written), Ok(first), Ok(second)) = (
        <&mut [u8; PAGE_SIZE]>::try_from(written),
        <&mut [u8; PAGE_SIZE]>::try_from(first),
        <&mut [u8; PAGE_SIZE]>::try_from(second),
    ) else {
        unreachable!("the scratch is three pages");
    };
    let total = blocks.iter().map(|&(_, pages)| pages).sum::<u64>() * STEP_COUNT;
    let mut outcome = Outcome { complete: false, errors: 0, bad: Bad::new(room), unplaced: 0 };
    let mut done = 0;
    for (step, &(check, write)) in STEPS.iter().enumerate() {
        for &(start, count) in blocks {
            for page in 0..count {
                let at = start + page * PAGE_SIZE as u64;
                let address = |word: usize| at + 8 * word as u64;
                if let Some(fill) = check {
                    ram.read(at, first);
                    ram.read(at, second);
                    let wrong = (0..PAGE_WORDS)
                        .filter(|&at| {
                            let want = value(fill, seed, address(at));
                            word(first, at) != want || word(second, at) != want
                        })
                        .count();
                    let wrong = u32::try_from(wrong).unwrap_or(u32::MAX);
                    if wrong > 0 {
                        outcome.errors += u64::from(wrong);
                        if !outcome.bad.add(at, wrong) {
                            outcome.unplaced += u64::from(wrong);
                        }
                    }
                }
                if let Some(fill) = write {
                    for (at, bytes) in written.as_chunks_mut::<8>().0.iter_mut().enumerate() {
                        *bytes = value(fill, seed, address(at)).to_le_bytes();
                    }
                    ram.write(at, written);
                }
            }
            done += count;
            progress(done, total);
            if out_of_time() {
                return outcome;
            }
        }
        if step == 0
            && fault
            && let Some(&(at, _)) = blocks.iter().find(|&&(_, count)| count > 0)
        {
            ram.read(at, first);
            first[2] ^= 1 << 1;
            ram.write(at, first);
        }
    }
    outcome.complete = true;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::vec::Vec;

    /// Memory in a map of words, with the faults a test asks for.
    #[derive(Default)]
    struct Fake {
        words: BTreeMap<u64, u64>,
        /// Words that always read this value, whatever is written.
        stuck: BTreeMap<u64, u64>,
        /// A bit of the address that the memory does not decode: two
        /// addresses that differ in it reach one word.
        ignored_bit: Option<u32>,
        /// A word whose first read after each write has this bit flipped.
        flaky: Option<(u64, u32)>,
        flaky_read: bool,
    }

    impl Fake {
        fn cell(&self, address: u64) -> u64 {
            self.ignored_bit.map_or(address, |bit| address & !(1 << bit))
        }
    }

    impl Ram for Fake {
        fn write(&mut self, at: u64, bytes: &[u8; PAGE_SIZE]) {
            for word in 0..PAGE_WORDS {
                let address = at + 8 * word as u64;
                let cell = self.cell(address);
                self.words.insert(cell, super::word(bytes, word));
                if self.flaky.is_some_and(|(flaky, _)| flaky == address) {
                    self.flaky_read = true;
                }
            }
        }

        fn read(&mut self, at: u64, bytes: &mut [u8; PAGE_SIZE]) {
            for (word, slot) in bytes.as_chunks_mut::<8>().0.iter_mut().enumerate() {
                let address = at + 8 * word as u64;
                let mut value = match self.stuck.get(&address) {
                    Some(&value) => value,
                    None => self.words.get(&self.cell(address)).copied().unwrap_or(0),
                };
                if let Some((flaky, bit)) = self.flaky
                    && flaky == address
                    && self.flaky_read
                {
                    value ^= 1 << bit;
                    self.flaky_read = false;
                }
                *slot = value.to_le_bytes();
            }
        }
    }

    const PAGE: u64 = PAGE_SIZE as u64;

    /// Runs a test of `blocks` on `ram`, with `fault`, and gives how it
    /// ended: whether complete, the errors, the frames, the unplaced.
    fn test(
        ram: &mut Fake,
        blocks: &[(u64, u64)],
        fault: bool,
    ) -> (bool, u64, Vec<(u64, u32)>, u64) {
        let mut scratch = [0u8; SCRATCH];
        let mut room = std::vec![(0, 0); BAD_MAX];
        let test = Test { blocks, seed: 7, fault, scratch: &mut scratch, room: &mut room };
        let outcome = run(ram, test, || false, |_, _| {});
        (outcome.complete, outcome.errors, outcome.bad.frames().to_vec(), outcome.unplaced)
    }

    #[test]
    fn good_memory_has_no_errors_and_every_step_reaches_every_page() {
        let mut ram = Fake::default();
        let blocks = [(0x10_0000, 4), (0x40_0000, 2)];
        let mut told = Vec::new();
        let mut scratch = [0u8; SCRATCH];
        let mut room = [(0, 0); 4];
        let test =
            Test { blocks: &blocks, seed: 7, fault: false, scratch: &mut scratch, room: &mut room };
        let outcome = run(&mut ram, test, || false, |done, total| told.push((done, total)));
        assert!(outcome.complete);
        assert_eq!((outcome.errors, outcome.bad.frames().len()), (0, 0));
        assert_eq!(told.len(), blocks.len() * STEPS.len());
        assert_eq!(told.last(), Some(&(6 * STEP_COUNT, 6 * STEP_COUNT)));
        // The last step's random words, which another seed changes.
        let at = 0x10_0000 + 8;
        assert_eq!(ram.words[&at], random(7, at));
        assert_ne!(random(7, at), random(8, at));
        assert_ne!(random(7, at), random(7, at + 8));
    }

    #[test]
    fn a_bit_flipped_after_it_was_written_is_found_in_its_frame() {
        let mut ram = Fake::default();
        assert_eq!(
            test(&mut ram, &[(0x20_0000, 3)], true),
            (true, 1, std::vec![(0x20_0000, 1)], 0)
        );
    }

    #[test]
    fn a_stuck_word_is_found_by_every_step_that_reads_it() {
        let stuck = 0x30_0000 + PAGE + 16;
        for value in [0, u64::MAX] {
            let mut ram = Fake::default();
            ram.stuck.insert(stuck, value);
            // Each of the five steps that read sees it wrong.
            let (complete, errors, frames, _) = test(&mut ram, &[(0x30_0000, 2)], false);
            assert!(complete);
            assert_eq!((errors, frames), (5, std::vec![(0x30_0000 + PAGE, 5)]), "{value:#x}");
        }
    }

    #[test]
    fn two_addresses_that_reach_one_word_are_found_by_every_step() {
        // Two pages that differ in bit 12 only reach the same words. The
        // first reads the second's addresses, and later its random words;
        // the second reads, in every step, what the first wrote after
        // its own check.
        let mut ram = Fake { ignored_bit: Some(12), ..Fake::default() };
        let (_, errors, frames, _) = test(&mut ram, &[(0x40_0000, 2)], false);
        let words = PAGE_WORDS as u32;
        assert_eq!(frames, [(0x40_0000, 2 * words), (0x40_0000 + PAGE, 4 * words)]);
        assert_eq!(errors, 6 * u64::from(words));
    }

    #[test]
    fn a_read_that_fails_once_is_an_error() {
        let flaky = 0x50_0000 + 64;
        let mut ram = Fake { flaky: Some((flaky, 3)), ..Fake::default() };
        // The first read after each of the five writes that a check
        // follows.
        assert_eq!(
            test(&mut ram, &[(0x50_0000, 1)], false),
            (true, 5, std::vec![(0x50_0000, 5)], 0)
        );
    }

    #[test]
    fn frames_next_to_each_other_are_one_range() {
        let mut room = [(0, 0); 8];
        let mut bad = Bad::new(&mut room);
        for (frame, errors) in [(3, 1), (1, 2), (2, 1), (7, 4), (9, 1), (3, 1)] {
            assert!(bad.add(frame * PAGE, errors));
        }
        let ranges: Vec<_> = bad.ranges().collect();
        assert_eq!(
            ranges,
            [(PAGE, 4 * PAGE - 1, 5), (7 * PAGE, 8 * PAGE - 1, 4), (9 * PAGE, 10 * PAGE - 1, 1)]
        );
    }

    #[test]
    fn at_most_sixty_four_ranges_are_said_and_the_room_is_kept_to() {
        let mut room = std::vec![(0, 0); BAD_MAX];
        let mut bad = Bad::new(&mut room);
        for frame in 0..BAD_MAX as u64 {
            assert!(bad.add(2 * frame * PAGE, 1));
        }
        assert!(!bad.add(PAGE, 1));
        assert!(bad.add(0, 1), "a frame kept already still counts");
        assert_eq!(bad.frames().len(), BAD_MAX);
        assert_eq!(bad.ranges().count(), RANGES_MAX);
        assert_eq!(bad.ranges().next(), Some((0, PAGE - 1, 2)));
    }

    #[test]
    fn errors_past_the_room_are_counted_but_not_placed() {
        let mut ram = Fake::default();
        for page in 0..=BAD_MAX as u64 {
            ram.stuck.insert(page * PAGE + 8, 1);
        }
        let (_, errors, frames, unplaced) = test(&mut ram, &[(0, BAD_MAX as u64 + 1)], false);
        assert_eq!(frames.len(), BAD_MAX);
        assert_eq!(unplaced, 5);
        assert_eq!(errors, 5 * (BAD_MAX as u64 + 1));
    }

    #[test]
    fn the_test_stops_when_its_time_is_out() {
        let mut ram = Fake::default();
        let mut asked = 0;
        let mut scratch = [0u8; SCRATCH];
        let mut room = [(0, 0); 4];
        let blocks = [(0, 1), (PAGE, 1), (2 * PAGE, 1)];
        let test =
            Test { blocks: &blocks, seed: 7, fault: false, scratch: &mut scratch, room: &mut room };
        let outcome = run(
            &mut ram,
            test,
            || {
                asked += 1;
                asked == 4
            },
            |_, _| {},
        );
        assert!(!outcome.complete);
        assert_eq!((asked, outcome.errors), (4, 0));
    }
}
