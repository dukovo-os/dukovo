// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The read scan of a disk: reads the disk through its device file in
//! pieces of 4 MiB, 256 of them spread evenly over the disk in a quick
//! check and all of them in a full one, and narrows a piece that cannot
//! be read to the sectors that cannot, by reading halves of it.
//!
//! The reading itself is a function the caller gives, which answers how
//! long a read took, so that the tests plant bad sectors and slow reads.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How much one read takes.
pub(crate) const PIECE: u64 = 4 << 20;

/// How many pieces a quick scan reads.
const QUICK_PIECES: u64 = 256;

/// A read of one piece that takes longer than this is slow: a disk that
/// reads 4 MiB in 2 seconds gives 2 MiB a second.
pub(crate) const SLOW: Duration = Duration::from_secs(2);

/// How many ranges of unreadable sectors the scan names; those after
/// them are only counted.
pub(crate) const MAX_RANGES: usize = 10;

/// Sectors that cannot be read, from `first` to `last`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Range {
    pub(crate) first: u64,
    pub(crate) last: u64,
}

/// What a scan found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Scan {
    /// The bytes read without a failure.
    pub(crate) bytes: u64,
    /// The first ranges of sectors that cannot be read.
    pub(crate) unreadable: Vec<Range>,
    /// How many sectors in those ranges.
    pub(crate) sectors: u64,
    /// The pieces with unreadable sectors that were not narrowed, since
    /// [`MAX_RANGES`] were named already.
    pub(crate) unnarrowed: u64,
    /// The pieces that failed and then read whole, in halves.
    pub(crate) retried: u64,
    /// The reads that took longer than [`SLOW`], and the longest read.
    pub(crate) slow: u64,
    pub(crate) slowest: Duration,
    /// Whether the person stopped the scan before its end.
    pub(crate) stopped: bool,
}

impl Scan {
    /// Notes that `sector` cannot be read: it extends the last range when
    /// it follows it.
    fn unreadable_sector(&mut self, sector: u64) {
        self.sectors += 1;
        match self.unreadable.last_mut() {
            Some(range) if range.last + 1 == sector => range.last = sector,
            _ => self.unreadable.push(Range { first: sector, last: sector }),
        }
    }
}

/// The places of the pieces to read on a disk of `size` bytes, as offset
/// and length: all of the disk for a full scan, else at most
/// [`QUICK_PIECES`] spread evenly, which are all of a disk that has no
/// more room than that. Offsets are multiples of `sector`.
pub(crate) fn pieces(size: u64, sector: u64, full: bool) -> impl Iterator<Item = (u64, u64)> {
    let all = size.div_ceil(PIECE);
    let count = if full { all } else { all.min(QUICK_PIECES) };
    let step = if count == all { PIECE } else { size / count / sector * sector };
    (0..count).map(move |number| {
        let offset = number * step;
        (offset, PIECE.min(size - offset))
    })
}

/// Reads `pieces` with `read`, which gives how long reading `length`
/// bytes at `offset` took. A piece that fails is read again in halves,
/// down to single sectors, to find the sectors that cannot be read. Ends
/// early when `stop` is set.
pub(crate) fn scan(
    pieces: impl Iterator<Item = (u64, u64)>,
    sector: u64,
    read: &mut dyn FnMut(u64, u64) -> io::Result<Duration>,
    stop: &AtomicBool,
) -> Scan {
    let mut found = Scan::default();
    for (offset, length) in pieces {
        if stop.load(Ordering::Relaxed) {
            found.stopped = true;
            break;
        }
        match read(offset, length) {
            Ok(took) => {
                found.bytes += length;
                if took > SLOW {
                    found.slow += 1;
                }
                found.slowest = found.slowest.max(took);
            }
            Err(_) => {
                let before = (found.sectors, found.unnarrowed);
                narrow(offset, length, sector, read, &mut found);
                if (found.sectors, found.unnarrowed) == before {
                    found.retried += 1;
                }
            }
        }
    }
    found
}

/// Reads the halves of the piece that failed at `offset`, and theirs if
/// they fail, down to a sector, which is then unreadable.
fn narrow(
    offset: u64,
    length: u64,
    sector: u64,
    read: &mut dyn FnMut(u64, u64) -> io::Result<Duration>,
    found: &mut Scan,
) {
    if found.unreadable.len() >= MAX_RANGES {
        found.unnarrowed += 1;
        return;
    }
    if length <= sector {
        found.unreadable_sector(offset / sector);
        return;
    }
    // The first half is a whole number of sectors, so that every read
    // starts on a sector.
    let first = (length / 2).div_ceil(sector) * sector;
    for (at, part) in [(offset, first), (offset + first, length - first)] {
        match read(at, part) {
            Ok(_) => found.bytes += part,
            Err(_) => narrow(at, part, sector, read, found),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECTOR: u64 = 512;

    fn running() -> AtomicBool {
        AtomicBool::new(false)
    }

    /// A disk whose reads fail where they touch one of `bad` sectors and
    /// take `slow_at` seconds where they start at an offset it names.
    fn disk<'a>(
        bad: &'a [u64],
        slow_at: &'a [(u64, u64)],
    ) -> impl FnMut(u64, u64) -> io::Result<Duration> + 'a {
        move |offset, length| {
            let touched = |sector: &u64| (offset..offset + length).contains(&(sector * SECTOR));
            if bad.iter().any(touched) {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            let slow = slow_at.iter().find(|(at, _)| *at == offset).map(|(_, seconds)| *seconds);
            Ok(Duration::from_secs(slow.unwrap_or(0)))
        }
    }

    #[test]
    fn a_small_disk_is_read_whole_in_pieces_of_4_mib() {
        let all: Vec<_> = pieces(10 << 20, SECTOR, false).collect();
        assert_eq!(all, [(0, PIECE), (PIECE, PIECE), (2 * PIECE, 2 << 20)]);
        assert_eq!(pieces(10 << 20, SECTOR, true).count(), 3);
        assert_eq!(pieces(PIECE, SECTOR, false).collect::<Vec<_>>(), [(0, PIECE)]);
    }

    #[test]
    fn a_big_disk_has_256_pieces_spread_evenly_and_a_full_scan_all() {
        let size = 2 << 40;
        let quick: Vec<_> = pieces(size, SECTOR, false).collect();
        assert_eq!(quick.len(), 256);
        assert_eq!(quick[0], (0, PIECE));
        assert_eq!(quick[1].0 - quick[0].0, size / 256);
        assert!(
            quick.iter().all(|(offset, length)| offset % SECTOR == 0 && offset + length <= size)
        );
        assert_eq!(pieces(size, SECTOR, true).count() as u64, size / PIECE);
    }

    #[test]
    fn pieces_start_on_the_sectors_of_a_disk_with_big_ones() {
        // 4096-byte sectors, and a size that 256 does not divide into them.
        let size = 3_000_000_000_000 / 4096 * 4096;
        assert!(pieces(size, 4096, false).all(|(offset, _)| offset % 4096 == 0));
    }

    #[test]
    fn a_good_disk_reads_everything_and_names_nothing() {
        let mut good = disk(&[], &[]);
        let found = scan(pieces(10 << 20, SECTOR, false), SECTOR, &mut good, &running());
        assert_eq!(found.bytes, 10 << 20);
        assert_eq!((found.slow, found.retried), (0, 0));
        assert!(found.unreadable.is_empty() && !found.stopped);
    }

    #[test]
    fn a_piece_that_fails_is_narrowed_to_its_sectors() {
        // Sectors 20000 and 20001 are side by side and make one range;
        // 40000 is another.
        let bad = [20000, 20001, 40000];
        let mut bad_disk = disk(&bad, &[]);
        let found = scan(pieces(64 << 20, SECTOR, false), SECTOR, &mut bad_disk, &running());
        assert_eq!(
            found.unreadable,
            [Range { first: 20000, last: 20001 }, Range { first: 40000, last: 40000 }]
        );
        assert_eq!(found.bytes, (64 << 20) - 3 * SECTOR);
        assert_eq!((found.sectors, found.unnarrowed, found.retried), (3, 0, 0));
    }

    #[test]
    fn a_bad_sector_at_the_edge_of_a_piece_is_found() {
        let size = 8 << 20;
        let first_of_second_piece = PIECE / SECTOR;
        let bad = [first_of_second_piece, size / SECTOR - 1, 0];
        let mut bad_disk = disk(&bad, &[]);
        let found = scan(pieces(size, SECTOR, false), SECTOR, &mut bad_disk, &running());
        let sectors: Vec<(u64, u64)> = found.unreadable.iter().map(|r| (r.first, r.last)).collect();
        assert_eq!(
            sectors,
            [
                (0, 0),
                (first_of_second_piece, first_of_second_piece),
                (size / SECTOR - 1, size / SECTOR - 1)
            ]
        );
    }

    #[test]
    fn only_the_first_ranges_are_named_and_the_rest_counted() {
        // Every 100th sector of the first 64 MiB is bad: 1311 ranges.
        let bad: Vec<u64> = (0..131_072).step_by(100).collect();
        let mut bad_disk = disk(&bad, &[]);
        let found = scan(pieces(64 << 20, SECTOR, false), SECTOR, &mut bad_disk, &running());
        assert_eq!(found.unreadable.len(), MAX_RANGES);
        assert!(found.unnarrowed > 0);
        // The ranges are the lowest ones.
        assert_eq!(found.unreadable[0], Range { first: 0, last: 0 });
        assert_eq!(found.unreadable[1], Range { first: 100, last: 100 });
    }

    #[test]
    fn a_piece_that_fails_once_and_then_reads_is_counted_as_retried() {
        let mut failed_once = false;
        let mut read = |_offset: u64, _length: u64| {
            if failed_once {
                Ok(Duration::ZERO)
            } else {
                failed_once = true;
                Err(io::Error::from_raw_os_error(libc::EIO))
            }
        };
        let found = scan(pieces(PIECE, SECTOR, false), SECTOR, &mut read, &running());
        assert_eq!((found.retried, found.bytes), (1, PIECE));
        assert!(found.unreadable.is_empty());
    }

    #[test]
    fn reads_over_two_seconds_are_slow_and_the_slowest_is_kept() {
        let mut slow_disk = disk(&[], &[(0, 3), (PIECE, 2), (2 * PIECE, 9)]);
        let found = scan(pieces(12 << 20, SECTOR, false), SECTOR, &mut slow_disk, &running());
        assert_eq!(found.slow, 2);
        assert_eq!(found.slowest, Duration::from_secs(9));
    }

    #[test]
    fn a_scan_the_person_stops_ends_at_once() {
        let stop = AtomicBool::new(true);
        let mut good = disk(&[], &[]);
        let found = scan(pieces(10 << 20, SECTOR, false), SECTOR, &mut good, &stop);
        assert!(found.stopped);
        assert_eq!(found.bytes, 0);
    }
}
