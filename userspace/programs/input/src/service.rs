// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What inputd(8) and a program that holds the screens say to each other
//! (ADR 0707).
//!
//! inputd hands every input device to whoever holds a screen, and reads
//! no event itself. A program connects to [`SOCKET`], a stream socket,
//! and both ends write [`Record`]s, 16 bytes each, numbers in
//! little-endian order:
//!
//! 1. The program sends [`HOLDER`], whose number is [`VERSION`], and
//!    with it (SCM_RIGHTS) a copy of its description of the display
//!    layer's file (radix-display(2)). The copy needs no right: it only
//!    names the holder.
//! 2. inputd sends a [`DEVICE`] for each device there is, with the
//!    device's number and its [`Kinds`], and with each (SCM_RIGHTS) a
//!    description of the device's file, _/devices/input/N_, that it
//!    opened for this program and tied to the holder (radix-input(2)'s
//!    INPUT_TIE). It reads events only while that holder is in front.
//!    Then [`READY`], whose number is how many devices were sent.
//! 3. From then on inputd sends a [`DEVICE`] when a device comes and a
//!    [`GONE`] when one goes. The program says nothing more, and closes
//!    the connection when it wants no further device.
//!
//! inputd answers [`REFUSED`], with a REFUSED_* number, and closes the
//! connection if the first record is not a [`HOLDER`] of a version it
//! knows, if what came with it is not a holder of the display layer, or
//! if too many programs are connected. A program that does not read
//! what it is sent is disconnected.
//!
//! A descriptor comes with the first byte of its record. [`Inbox`] puts
//! what a program reads back together, however the bytes were cut.

use std::collections::VecDeque;

use crate::kind::Kinds;

/// Where inputd listens.
pub const SOCKET: &str = "/system/live/inputd/devices";

/// The version of this exchange: a [`HOLDER`]'s number.
pub const VERSION: u32 = 1;

/// inputd's: a device, with a description of its file. The number is the
/// device's, the N of _/devices/input/N_, and the kind its [`Kinds`].
pub const DEVICE: u32 = 1;
/// inputd's: the device with this number went. Its file reads nothing
/// more.
pub const GONE: u32 = 2;
/// inputd's: every device there was has been sent. The number is how
/// many.
pub const READY: u32 = 3;
/// inputd's: no device is handed out, and the connection is closed. The
/// number is a REFUSED_*.
pub const REFUSED: u32 = 4;
/// The program's: it holds a screen, and a copy of its description of
/// the display layer's file comes with this. The number is [`VERSION`].
pub const HOLDER: u32 = 5;

/// Why inputd refused: what came with [`HOLDER`] is not a holder of the
/// display layer, or nothing came with it.
pub const REFUSED_NOT_A_HOLDER: u32 = 1;
/// The first record was not a [`HOLDER`] of a version inputd knows.
pub const REFUSED_VERSION: u32 = 2;
/// As many programs as inputd serves are connected.
pub const REFUSED_FULL: u32 = 3;

/// One record of the exchange.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    /// What it says: [`DEVICE`], [`GONE`], [`READY`], [`REFUSED`] or
    /// [`HOLDER`].
    pub what: u32,
    /// The device's number, or what `what` says it is.
    pub number: u32,
    /// With [`DEVICE`], what the device is; 0 otherwise.
    pub kind: u32,
}

impl Record {
    /// A record's bytes: `what`, `number`, `kind` and four bytes that
    /// are always 0.
    pub const SIZE: usize = 16;

    /// The program's first record.
    pub const fn holder() -> Record {
        Record { what: HOLDER, number: VERSION, kind: 0 }
    }

    /// The device `number`, which is `kinds`.
    pub const fn device(number: u32, kinds: Kinds) -> Record {
        Record { what: DEVICE, number, kind: kinds.bits() }
    }

    /// The device `number` went.
    pub const fn gone(number: u32) -> Record {
        Record { what: GONE, number, kind: 0 }
    }

    /// `count` devices were sent, which are all there were.
    pub const fn ready(count: u32) -> Record {
        Record { what: READY, number: count, kind: 0 }
    }

    /// Refused, for `reason`, a REFUSED_*.
    pub const fn refused(reason: u32) -> Record {
        Record { what: REFUSED, number: reason, kind: 0 }
    }

    /// With [`DEVICE`], what the device is.
    pub fn kinds(&self) -> Kinds {
        Kinds::from_bits(self.kind)
    }

    /// Its bytes.
    pub fn to_bytes(&self) -> [u8; Record::SIZE] {
        let mut out = [0; Record::SIZE];
        out[..4].copy_from_slice(&self.what.to_le_bytes());
        out[4..8].copy_from_slice(&self.number.to_le_bytes());
        out[8..12].copy_from_slice(&self.kind.to_le_bytes());
        out
    }

    /// The record `bytes` are.
    pub fn from_bytes(bytes: &[u8; Record::SIZE]) -> Record {
        let word = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        Record { what: word(0), number: word(4), kind: word(8) }
    }
}

/// What a program reads from inputd, put back together: each record,
/// and with a [`DEVICE`] the description that came with it.
///
/// The program reads the socket with recvmsg(2) and gives every read's
/// bytes and descriptors to [`Inbox::take`], then takes the records out
/// with [`Inbox::next`]. `F` is the descriptor, an `OwnedFd`.
#[derive(Debug)]
pub struct Inbox<F> {
    /// The bytes of a record that has not all come.
    partial: Vec<u8>,
    /// The whole records, oldest first.
    records: VecDeque<Record>,
    /// The descriptors no record has taken yet, oldest first.
    files: VecDeque<F>,
}

impl<F> Default for Inbox<F> {
    fn default() -> Inbox<F> {
        Inbox::new()
    }
}

/// A [`DEVICE`] whose description did not come with it: inputd, or what
/// answers in its place, does not keep to the exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoFile(pub Record);

impl core::fmt::Display for NoFile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "input device {} came without its file", self.0.number)
    }
}

impl std::error::Error for NoFile {}

impl<F> Inbox<F> {
    /// An inbox nothing has come to.
    pub fn new() -> Inbox<F> {
        Inbox { partial: Vec::new(), records: VecDeque::new(), files: VecDeque::new() }
    }

    /// Takes in what one read returned: its bytes and the descriptors
    /// that came with them.
    pub fn take(&mut self, bytes: &[u8], files: impl IntoIterator<Item = F>) {
        self.files.extend(files);
        let mut rest = bytes;
        while !rest.is_empty() {
            let want = Record::SIZE - self.partial.len();
            let (now, later) = rest.split_at(want.min(rest.len()));
            self.partial.extend_from_slice(now);
            rest = later;
            if let Ok(whole) = <&[u8; Record::SIZE]>::try_from(self.partial.as_slice()) {
                self.records.push_back(Record::from_bytes(whole));
                self.partial.clear();
            }
        }
    }

    /// The next whole record, and the description that came with it if
    /// it is a [`DEVICE`]; `None` when no whole record waits.
    ///
    /// # Errors
    ///
    /// [`NoFile`] for a [`DEVICE`] with no description.
    #[allow(clippy::should_implement_trait, reason = "an inbox is read, not looped over")]
    pub fn next(&mut self) -> Option<Result<(Record, Option<F>), NoFile>> {
        let record = self.records.pop_front()?;
        if record.what != DEVICE {
            return Some(Ok((record, None)));
        }
        Some(self.files.pop_front().map(|file| (record, Some(file))).ok_or(NoFile(record)))
    }
}
