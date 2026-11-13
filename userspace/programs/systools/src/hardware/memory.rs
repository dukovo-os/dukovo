// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The memory area of `hardware check`: the memory errors the machine-check
//! banks counted since the start (`/kernel/devices/system/edac`, ADR
//! 0169), and a test of a share of the free memory while the system runs.
//!
//! The test takes anonymous memory and has one thread for each CPU test a
//! slice of it with three kinds of pattern: each word's own address, moving
//! inversions of 0x55 and 0xAA, and random values from a seed. Each is
//! written and read back twice, so that a word that flips between two
//! reads shows too. A running system cannot test the memory it uses
//! itself, so the result says that Test the memory in the start menu
//! (memtest86+) tests all of it.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use super::check::{Area, Check, Problem, counted};
use crate::util;

/// Where the kernel counts the memory controller's errors.
pub(crate) const EDAC: &str = "/kernel/devices/system/edac/mc/mc0";

/// How many times the whole region is read or written in a run of every
/// pattern: the address pattern's three, the two inversions' three each,
/// and the random pattern's three.
pub(crate) const SWEEPS: u64 = 12;

/// How often, in words, a pattern looks whether the person stopped the run.
const STOP_EVERY: usize = 1 << 16;

/// The most memory a quick check tests.
const QUICK_MAX: u64 = 1 << 30;

/// Memory as a pattern reads and writes it. The test's own implementation
/// is a faulty chip.
pub(crate) trait Words {
    fn count(&self) -> usize;
    fn read(&self, index: usize) -> u64;
    fn write(&mut self, index: usize, value: u64);
}

impl Words for [u64] {
    fn count(&self) -> usize {
        self.len()
    }

    fn read(&self, index: usize) -> u64 {
        // SAFETY: the indexing checked the bounds, so the reference is
        // valid and aligned. The access is volatile so that the compiler
        // does not answer a read from the write before it.
        unsafe { std::ptr::read_volatile(&raw const self[index]) }
    }

    fn write(&mut self, index: usize, value: u64) {
        // SAFETY: as for the read.
        unsafe { std::ptr::write_volatile(&raw mut self[index], value) }
    }
}

/// A word that read back as something else than was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Wrong {
    pub(crate) address: u64,
    pub(crate) wanted: u64,
    pub(crate) read: u64,
    /// The pattern that found it.
    pub(crate) pattern: &'static str,
}

/// The state of a pattern's random numbers (xorshift64): a seed of 0
/// would stay 0.
fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// A run of the patterns over some memory that starts at `base`.
struct Sweep<'a, W: Words + ?Sized> {
    words: &'a mut W,
    base: u64,
    stop: &'a AtomicBool,
}

impl<W: Words + ?Sized> Sweep<'_, W> {
    fn check(&self, index: usize, wanted: u64, pattern: &'static str) -> Result<(), Wrong> {
        let read = self.words.read(index);
        if read == wanted {
            return Ok(());
        }
        Err(Wrong { address: self.base + 8 * index as u64, wanted, read, pattern })
    }

    /// Whether the person stopped the run, looked at once in a while.
    fn halted(&self, index: usize) -> bool {
        index.is_multiple_of(STOP_EVERY) && self.stop.load(Ordering::Relaxed)
    }

    /// Each word holds its own address: a fault in an address line shows
    /// as words that read back what another one holds.
    fn address(&mut self) -> Result<(), Wrong> {
        const PATTERN: &str = "each word's own address";
        for index in 0..self.words.count() {
            if self.halted(index) {
                return Ok(());
            }
            self.words.write(index, self.base + 8 * index as u64);
        }
        for _ in 0..2 {
            for index in 0..self.words.count() {
                if self.halted(index) {
                    return Ok(());
                }
                self.check(index, self.base + 8 * index as u64, PATTERN)?;
            }
        }
        Ok(())
    }

    /// Writes `pattern`, then goes up the memory checking each word and
    /// writing its inverse, and down it checking the inverse and writing
    /// `pattern` back, so that a bit that cannot change shows both ways.
    fn inversions(&mut self, pattern: u64) -> Result<(), Wrong> {
        const NAME: &str = "moving inversions";
        let count = self.words.count();
        for index in 0..count {
            if self.halted(index) {
                return Ok(());
            }
            self.words.write(index, pattern);
        }
        for index in 0..count {
            if self.halted(index) {
                return Ok(());
            }
            self.check(index, pattern, NAME)?;
            self.words.write(index, !pattern);
        }
        for index in (0..count).rev() {
            if self.halted(index) {
                return Ok(());
            }
            self.check(index, !pattern, NAME)?;
            self.words.write(index, pattern);
        }
        Ok(())
    }

    /// Random values from `seed`, written once and read back twice.
    fn random(&mut self, seed: u64) -> Result<(), Wrong> {
        let mut state = seed | 1;
        for index in 0..self.words.count() {
            if self.halted(index) {
                return Ok(());
            }
            self.words.write(index, next_random(&mut state));
        }
        for _ in 0..2 {
            let mut state = seed | 1;
            for index in 0..self.words.count() {
                if self.halted(index) {
                    return Ok(());
                }
                self.check(index, next_random(&mut state), "random values")?;
            }
        }
        Ok(())
    }
}

/// Runs every pattern over `words`, which start at address `base`, and
/// gives the first word that read back wrong. A run that the person
/// stops ends early, with nothing wrong found.
pub(crate) fn run_patterns<W: Words + ?Sized>(
    words: &mut W,
    base: u64,
    seed: u64,
    stop: &AtomicBool,
) -> Result<(), Wrong> {
    let mut sweep = Sweep { words, base, stop };
    sweep.address()?;
    sweep.inversions(0x5555_5555_5555_5555)?;
    sweep.inversions(0xaaaa_aaaa_aaaa_aaaa)?;
    sweep.random(seed)
}

/// Tests `region` with `threads` threads, each its own slice, and gives
/// the first wrong word found, the lowest address first.
pub(crate) fn test_region(
    region: &mut [u64],
    threads: usize,
    seed: u64,
    stop: &AtomicBool,
) -> Option<Wrong> {
    let slice_len = region.len().div_ceil(threads.max(1)).max(1);
    let mut found: Vec<Wrong> = thread::scope(|scope| {
        let workers: Vec<_> = region
            .chunks_mut(slice_len)
            .enumerate()
            .map(|(number, slice)| {
                let base = slice.as_ptr() as u64;
                scope.spawn(move || run_patterns(slice, base, seed ^ number as u64, stop))
            })
            .collect();
        workers
            .into_iter()
            .filter_map(|worker| match worker.join() {
                Ok(done) => done.err(),
                Err(panic) => std::panic::resume_unwind(panic),
            })
            .collect()
    });
    found.sort_by_key(|wrong| wrong.address);
    found.into_iter().next()
}

/// How many bytes of the free memory a check tests: a quarter of it, or
/// at most 1 GiB, quick, and three quarters of it for a full check.
pub(crate) fn tested_bytes(available_kib: u64, full: bool) -> u64 {
    let available = available_kib.saturating_mul(1024);
    if full { available / 4 * 3 } else { (available / 4).min(QUICK_MAX) }
}

/// What the memory controller counted since the start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Errors {
    pub(crate) corrected: u64,
    pub(crate) uncorrected: u64,
}

/// The counts in `dir` (Linux's EDAC layout), if the kernel keeps any.
pub(crate) fn read_errors(dir: &Path) -> Option<Errors> {
    let count = |file: &str| fs::read_to_string(dir.join(file)).ok()?.trim().parse().ok();
    Some(Errors { corrected: count("ce_count")?, uncorrected: count("ue_count")? })
}

/// The check of the memory errors: uncorrected ones fail it, corrected
/// ones warn.
pub(crate) fn errors_check(errors: Option<Errors>) -> Check {
    const NAME: &str = "memory errors";
    let Some(Errors { corrected, uncorrected }) = errors else {
        return Check::skipped(
            Area::Memory,
            NAME,
            "the kernel counts none on this machine: it reads no machine-check banks".into(),
        );
    };
    if uncorrected > 0 {
        let also =
            if corrected > 0 { format!(" and {corrected} corrected") } else { String::new() };
        let detail =
            format!("{}{also} counted since the start", counted(uncorrected, "uncorrected error"));
        return Check::failed(Area::Memory, NAME, Problem::MemoryUncorrected, detail);
    }
    if corrected > 0 {
        let detail = format!("{} counted since the start", counted(corrected, "corrected error"));
        return Check::warning(Area::Memory, NAME, Problem::MemoryCorrected, detail);
    }
    Check::passed(Area::Memory, NAME, "none counted since the start".into())
}

/// Takes `bytes` of anonymous memory and touches all of it, so that the
/// test reads what was written to real memory.
fn allocate(bytes: u64) -> Result<Vec<u64>, String> {
    let sorry = || format!("cannot take {} of memory", util::human(bytes, false));
    let words = usize::try_from(bytes / 8).map_err(|_| sorry())?;
    let mut region = Vec::new();
    region.try_reserve_exact(words).map_err(|_| sorry())?;
    region.resize(words, 0);
    Ok(region)
}

/// The check of the test: what it tested, or the first word that read
/// back wrong.
pub(crate) fn test_check(bytes: u64, threads: usize, found: Option<Wrong>, stopped: bool) -> Check {
    const NAME: &str = "memory test";
    if let Some(wrong) = found {
        // The address is the test's own, not the memory's: the start
        // menu's test names the module.
        let detail = format!(
            "a word read back {:#018x}, not {:#018x} (pattern: {})",
            wrong.read, wrong.wanted, wrong.pattern
        );
        return Check::failed(Area::Memory, NAME, Problem::MemoryTest, detail);
    }
    if stopped {
        return Check::skipped(Area::Memory, NAME, "stopped before it ended".into());
    }
    let cpus = if threads == 1 { "1 CPU".to_string() } else { format!("{threads} CPUs") };
    Check::passed(
        Area::Memory,
        NAME,
        format!(
            "{} tested by {cpus}; Test the memory in the start menu tests all of it",
            util::human(bytes, false)
        ),
    )
}

/// The checks of the memory area: the test of `bytes` of the free memory
/// by `threads` threads, then the errors counted, which include any that
/// the test caused.
pub(crate) fn run(bytes: u64, threads: usize, stop: &AtomicBool) -> Vec<Check> {
    let test = match allocate(bytes) {
        Ok(mut region) => {
            let seed = util::now_ns();
            let found = test_region(&mut region, threads, seed, stop);
            test_check(bytes, threads, found, stop.load(Ordering::Relaxed))
        }
        Err(why) => Check::skipped(Area::Memory, "memory test", why),
    };
    vec![test, errors_check(read_errors(Path::new(EDAC)))]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Memory with a word whose bits in `mask` cannot be changed: they
    /// read `stuck` (0 for the bits in `mask` stuck at 0).
    struct Faulty {
        words: Vec<u64>,
        at: usize,
        mask: u64,
        stuck: u64,
    }

    impl Faulty {
        fn new(len: usize, at: usize, mask: u64, stuck: u64) -> Faulty {
            Faulty { words: vec![0; len], at, mask, stuck }
        }
    }

    impl Words for Faulty {
        fn count(&self) -> usize {
            self.words.len()
        }

        fn read(&self, index: usize) -> u64 {
            self.words[index]
        }

        fn write(&mut self, index: usize, value: u64) {
            self.words[index] = if index == self.at {
                (value & !self.mask) | (self.stuck & self.mask)
            } else {
                value
            };
        }
    }

    fn running() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn good_memory_passes_every_pattern() {
        let mut words = vec![0u64; 4096];
        let base = words.as_ptr() as u64;
        assert_eq!(run_patterns(words.as_mut_slice(), base, 42, &running()), Ok(()));
        let mut faultless = Faulty::new(1000, 0, 0, 0);
        assert_eq!(run_patterns(&mut faultless, 0x10_0000, 7, &running()), Ok(()));
    }

    #[test]
    fn a_bit_stuck_at_zero_is_found_at_its_word() {
        // Bit 17 is 0 in the 0x55 pattern and 1 in the 0xAA one: the
        // second inversion pattern meets the bad word, at its address.
        let mut chip = Faulty::new(1000, 613, 1 << 17, 0);
        let wrong = run_patterns(&mut chip, 0x20_0000, 7, &running()).unwrap_err();
        assert_eq!(wrong.address, 0x20_0000 + 8 * 613);
        assert_eq!(wrong.pattern, "moving inversions");
        assert_eq!(wrong.wanted ^ wrong.read, 1 << 17);
    }

    #[test]
    fn a_bit_stuck_at_one_is_found_by_the_address_pattern() {
        // Bit 16 of an address this low is 0.
        let mut chip = Faulty::new(1000, 5, 1 << 16, 1 << 16);
        let wrong = run_patterns(&mut chip, 0, 7, &running()).unwrap_err();
        assert_eq!(wrong.address, 40);
        assert_eq!(wrong.wanted ^ wrong.read, 1 << 16);
        assert_eq!(wrong.pattern, "each word's own address");
    }

    #[test]
    fn the_random_pattern_alone_finds_a_word_that_holds_nothing() {
        let mut chip = Faulty::new(64, 3, u64::MAX, 0);
        let stop = running();
        let mut sweep = Sweep { words: &mut chip, base: 0x4000, stop: &stop };
        let wrong = sweep.random(9).unwrap_err();
        assert_eq!(wrong.address, 0x4000 + 24);
        assert_eq!(wrong.pattern, "random values");
        assert_eq!(wrong.read, 0);
    }

    #[test]
    fn a_stopped_run_ends_without_finding_anything() {
        let stop = AtomicBool::new(true);
        let mut chip = Faulty::new(1000, 5, 1, 0);
        assert_eq!(run_patterns(&mut chip, 0, 7, &stop), Ok(()));
        let check = test_check(1 << 20, 2, None, true);
        assert_eq!(check.outcome, super::super::check::Outcome::Skipped);
    }

    #[test]
    fn threads_test_a_slice_each_even_when_there_are_fewer_words_than_threads() {
        let mut region = vec![0u64; 10_000];
        assert_eq!(test_region(&mut region, 3, 1, &running()), None);
        assert_eq!(test_region(&mut region[..7], 4, 1, &running()), None);
        assert_eq!(test_region(&mut [], 2, 1, &running()), None);
    }

    #[test]
    fn a_quick_check_tests_a_quarter_up_to_a_gigabyte_and_a_full_one_three_quarters() {
        let gib = 1024 * 1024;
        assert_eq!(tested_bytes(400 * 1024, false), 100 * 1024 * 1024);
        assert_eq!(tested_bytes(16 * gib, false), 1 << 30);
        assert_eq!(tested_bytes(16 * gib, true), 12 << 30);
        assert_eq!(tested_bytes(0, true), 0);
    }

    #[test]
    fn the_errors_the_controller_counted_decide_the_check() {
        use super::super::check::Outcome;
        let outcome =
            |corrected, uncorrected| errors_check(Some(Errors { corrected, uncorrected })).outcome;
        assert_eq!(outcome(0, 0), Outcome::Passed);
        assert_eq!(outcome(3, 0), Outcome::Warning);
        assert_eq!(outcome(3, 1), Outcome::Failed);
        assert_eq!(outcome(0, 2), Outcome::Failed);
        let both = errors_check(Some(Errors { corrected: 3, uncorrected: 1 }));
        assert_eq!(both.id.as_deref(), Some("HW-102"));
        assert!(both.detail.starts_with("1 uncorrected error and 3 corrected"), "{}", both.detail);
        let few = errors_check(Some(Errors { corrected: 5, uncorrected: 0 }));
        assert_eq!(few.detail, "5 corrected errors counted since the start");
        assert_eq!(few.id.as_deref(), Some("HW-101"));
        assert_eq!(errors_check(None).outcome, Outcome::Skipped);
    }

    #[test]
    fn the_counts_are_read_from_the_edac_files() {
        let dir = std::env::temp_dir().join(format!("hardware-edac-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(read_errors(&dir), None);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("ce_count"), "4\n").unwrap();
        assert_eq!(read_errors(&dir), None);
        fs::write(dir.join("ue_count"), "0\n").unwrap();
        assert_eq!(read_errors(&dir), Some(Errors { corrected: 4, uncorrected: 0 }));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_wrong_word_fails_the_test_check_with_what_it_read() {
        let wrong = Wrong {
            address: 0x7f00_0000_1008,
            wanted: 0xaaaa_aaaa_aaaa_aaaa,
            read: 0xaaaa_aaaa_aaae_aaaa,
            pattern: "moving inversions",
        };
        let check = test_check(1 << 20, 2, Some(wrong), false);
        assert_eq!(check.id.as_deref(), Some("HW-103"));
        assert_eq!(
            check.detail,
            "a word read back 0xaaaaaaaaaaaeaaaa, not 0xaaaaaaaaaaaaaaaa (pattern: moving \
             inversions)"
        );
        let good = test_check(256 << 20, 2, None, false);
        assert_eq!(
            good.detail,
            "256M tested by 2 CPUs; Test the memory in the start menu tests all of it"
        );
    }
}
