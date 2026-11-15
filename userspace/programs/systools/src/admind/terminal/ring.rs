// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A pane's output as admind keeps it: the last 64 KiB, each byte known by
//! its offset in all the pane has written since it started. A page that
//! comes back with the offset it had seen gets what follows, while the
//! ring still holds it; else a replay from the first line the ring holds
//! whole.

use std::collections::VecDeque;

/// The bytes of output a pane keeps.
pub const RING_BYTES: usize = 64 << 10;

/// The last [`RING_BYTES`] of a pane's output.
#[derive(Debug)]
pub struct Ring {
    bytes: VecDeque<u8>,
    /// The offset just past the last byte: all the pane has written.
    end: u64,
}

impl Default for Ring {
    fn default() -> Ring {
        Ring { bytes: VecDeque::with_capacity(RING_BYTES), end: 0 }
    }
}

impl Ring {
    /// Keeps `output`, letting the oldest bytes go.
    pub fn push(&mut self, output: &[u8]) {
        // Only the last RING_BYTES of a long write are kept.
        let kept = &output[output.len().saturating_sub(RING_BYTES)..];
        let over = (self.bytes.len() + kept.len()).saturating_sub(RING_BYTES);
        self.bytes.drain(..over);
        self.bytes.extend(kept);
        self.end += output.len() as u64;
    }

    /// The offset just past the last byte written.
    pub fn end(&self) -> u64 {
        self.end
    }

    /// The offset of the oldest byte kept.
    pub fn start(&self) -> u64 {
        self.end - self.bytes.len() as u64
    }

    /// The bytes from `offset` to the end, if the ring still holds them
    /// all; an offset past the end is none the pane gave.
    pub fn since(&self, offset: u64) -> Option<Vec<u8>> {
        if offset < self.start() || offset > self.end {
            return None;
        }
        let from = usize::try_from(offset - self.start()).ok()?;
        Some(self.bytes.range(from..).copied().collect())
    }

    /// Where a replay starts: the first byte the ring holds when it holds
    /// the output from its first byte, else the byte after the first line
    /// end it holds, so that no line is drawn from its middle. A ring with
    /// no line end replays all it has.
    pub fn replay_from(&self) -> u64 {
        if self.start() == 0 {
            return 0;
        }
        match self.bytes.iter().position(|&byte| byte == b'\n') {
            Some(at) => self.start() + at as u64 + 1,
            None => self.start(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_kept_with_its_offsets_and_the_oldest_goes() {
        let mut ring = Ring::default();
        ring.push(b"hello\n");
        assert_eq!((ring.start(), ring.end()), (0, 6));
        assert_eq!(ring.since(0).unwrap(), b"hello\n");
        assert_eq!(ring.since(6).unwrap(), b"");
        assert_eq!(ring.since(7), None, "past the end");
        ring.push(&vec![b'x'; RING_BYTES]);
        assert_eq!((ring.start(), ring.end()), (6, 6 + RING_BYTES as u64));
        assert_eq!(ring.since(5), None, "no longer held");
        assert_eq!(ring.since(ring.end() - 2).unwrap(), b"xx");
    }

    #[test]
    fn a_write_longer_than_the_ring_keeps_its_end() {
        let mut ring = Ring::default();
        let mut long = vec![b'a'; RING_BYTES];
        long.extend_from_slice(b"tail");
        ring.push(&long);
        assert_eq!(ring.end(), long.len() as u64);
        assert_eq!(ring.start(), 4);
        assert!(ring.since(ring.start()).unwrap().ends_with(b"tail"));
    }

    #[test]
    fn a_replay_starts_at_the_first_whole_line() {
        let mut ring = Ring::default();
        ring.push(b"first\nsecond\n");
        assert_eq!(ring.replay_from(), 0, "the ring holds the output from its start");
        ring.push(&vec![b'y'; RING_BYTES - 4]);
        // "ond\n" and the y's are held: the replay skips "ond\n".
        assert_eq!(ring.since(ring.start()).unwrap()[..4], *b"ond\n");
        assert_eq!(ring.replay_from(), ring.start() + 4);
        ring.push(&vec![b'z'; RING_BYTES]);
        assert_eq!(ring.replay_from(), ring.start(), "no line end: all of it");
    }
}
