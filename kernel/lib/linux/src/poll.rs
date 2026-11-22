// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's `poll`, `ppoll`, `select` and `pselect6`: the events of a
//! `struct pollfd`, `select`'s descriptor sets, and the times they wait.
//!
//! The process's native `poll` takes Linux's `struct pollfd` array as it
//! is, with the events POSIX names: `POLLIN`, `POLLPRI`, `POLLOUT`,
//! `POLLERR`, `POLLHUP` and `POLLNVAL`. A program may also ask for the
//! normal and priority band events, which are the plain ones here, and
//! `select`'s sets become such an array. This is Dukovo's C library's
//! `poll.cpp` with Linux's raw values (a timeout left in `select`'s
//! `timeval`, for one).

use crate::records::Time;
use crate::sockets::{get, put};

/// `POLLIN`: there is something to read.
pub const POLLIN: u16 = 0x1;
/// `POLLPRI`: there is urgent data to read.
pub const POLLPRI: u16 = 0x2;
/// `POLLOUT`: writing will not wait.
pub const POLLOUT: u16 = 0x4;
/// `POLLERR`: an error.
pub const POLLERR: u16 = 0x8;
/// `POLLHUP`: the other end has closed.
pub const POLLHUP: u16 = 0x10;
/// `POLLNVAL`: the descriptor is not open.
pub const POLLNVAL: u16 = 0x20;
/// `POLLRDNORM`: normal data to read, which is `POLLIN` here.
pub const POLLRDNORM: u16 = 0x40;
/// `POLLRDBAND`: priority data to read, which is `POLLPRI` here.
pub const POLLRDBAND: u16 = 0x80;
/// `POLLWRNORM`: writing normal data will not wait, which is `POLLOUT`.
pub const POLLWRNORM: u16 = 0x100;
/// `POLLWRBAND`: writing priority data will not wait, which is `POLLOUT`.
pub const POLLWRBAND: u16 = 0x200;

/// The events the kernel knows, which are POSIX's basic ones.
const NATIVE: u16 = POLLIN | POLLPRI | POLLOUT | POLLERR | POLLHUP | POLLNVAL;

/// The size of a `struct pollfd`.
pub const POLLFD_SIZE: usize = 8;
/// The offset of `revents` in a `struct pollfd`.
pub const POLLFD_REVENTS: usize = 6;

/// A `struct pollfd`: a descriptor, the events asked for and the events
/// found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PollFd {
    /// The descriptor; negative to be skipped.
    pub fd: i32,
    /// The events asked for.
    pub events: u16,
    /// The events found.
    pub revents: u16,
}

impl PollFd {
    /// Reads a `struct pollfd`.
    pub fn read(bytes: &[u8; POLLFD_SIZE]) -> PollFd {
        PollFd {
            fd: i32::from_le_bytes(get(bytes, 0)),
            events: u16::from_le_bytes(get(bytes, 4)),
            revents: u16::from_le_bytes(get(bytes, POLLFD_REVENTS)),
        }
    }

    /// Writes a `struct pollfd`.
    pub fn put(&self, out: &mut [u8; POLLFD_SIZE]) {
        put(out, 0, self.fd.to_le_bytes());
        put(out, 4, self.events.to_le_bytes());
        put(out, POLLFD_REVENTS, self.revents.to_le_bytes());
    }
}

/// The events the kernel knows that stand for `events`: the normal and
/// priority band events are the plain ones.
pub fn native_events(events: u16) -> u16 {
    let mut out = events & NATIVE;
    if events & POLLRDNORM != 0 {
        out |= POLLIN;
    }
    if events & POLLRDBAND != 0 {
        out |= POLLPRI;
    }
    if events & (POLLWRNORM | POLLWRBAND) != 0 {
        out |= POLLOUT;
    }
    out
}

/// What a poll of `events` found, given what the kernel `found`: the
/// events asked for, always an error, a hang-up or an invalid descriptor,
/// and the band events asked for that stand for what it found.
pub fn found_events(events: u16, found: u16) -> u16 {
    let mut out = found & (events | POLLERR | POLLHUP | POLLNVAL);
    for (asked, plain, band) in [
        (POLLRDNORM, POLLIN, POLLRDNORM),
        (POLLRDBAND, POLLPRI, POLLRDBAND),
        (POLLWRNORM, POLLOUT, POLLWRNORM),
        (POLLWRBAND, POLLOUT, POLLWRBAND),
    ] {
        if events & asked != 0 && found & plain != 0 {
            out |= band;
        }
    }
    out
}

/// The most descriptors `select` takes, `FD_SETSIZE`.
pub const FD_SETSIZE: usize = 1024;

/// The bytes of a descriptor set for `nfds` descriptors: a whole number of
/// 64-bit words, as `select` reads them.
pub fn set_bytes(nfds: usize) -> usize {
    nfds.div_ceil(64) * 8
}

/// Whether descriptor `fd` is in `set`: bit `fd % 64` of word `fd / 64`.
pub fn in_set(set: &[u8], fd: usize) -> bool {
    set.get(fd / 8).is_some_and(|byte| byte >> (fd % 8) & 1 != 0)
}

/// Puts descriptor `fd` in `set`, if it has room for it.
pub fn add_to_set(set: &mut [u8], fd: usize) {
    if let Some(byte) = set.get_mut(fd / 8) {
        *byte |= 1 << (fd % 8);
    }
}

/// The events `select` asks for a descriptor that is in the sets `read`,
/// `write` and `except`.
pub fn select_events(read: bool, write: bool, except: bool) -> u16 {
    (if read { POLLIN } else { 0 })
        | (if write { POLLOUT } else { 0 })
        | (if except { POLLPRI } else { 0 })
}

/// Which of `select`'s sets a descriptor that asked for `events` is in
/// after the kernel `found` what it did: as on Linux, an end or an error
/// is something to read, and an error something to write. Returns the
/// read, write and except sets, in that order.
pub fn select_ready(events: u16, found: u16) -> [bool; 3] {
    [
        events & POLLIN != 0 && found & (POLLIN | POLLHUP | POLLERR) != 0,
        events & POLLOUT != 0 && found & (POLLOUT | POLLERR) != 0,
        events & POLLPRI != 0 && found & POLLPRI != 0,
    ]
}

/// A wait of `ms` milliseconds as `poll` takes it, in nanoseconds: `None`
/// for a negative one, which is for ever.
pub fn poll_wait(ms: i32) -> Option<u64> {
    u64::try_from(ms).ok().map(|ms| ms * 1_000_000)
}

/// A wait for `time`, in nanoseconds, as `ppoll`, `pselect6` and `select`
/// take it: `None`, for ever, if the program gave no time or one of more
/// than about 292 years, which no wait reaches.
pub fn wait_for(time: Option<Time>) -> Option<u64> {
    let time = time?;
    let limit = i64::MAX / 1_000_000_000 - 1;
    if time.sec >= limit {
        return None;
    }
    Some(time.as_ns())
}

/// The time a wait of `ns` nanoseconds still has to run, which `select`,
/// `pselect6` and `ppoll` write back as Linux does.
pub fn time_left(ns: u64) -> Time {
    Time::from_ns(ns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pollfd_has_linuxs_layout() {
        let mut bytes = [0u8; POLLFD_SIZE];
        PollFd { fd: -1, events: POLLIN | POLLOUT, revents: POLLHUP }.put(&mut bytes);
        assert_eq!(bytes, [0xff, 0xff, 0xff, 0xff, 5, 0, 0x10, 0]);
        assert_eq!(PollFd::read(&bytes), PollFd { fd: -1, events: 5, revents: 0x10 });
        assert_eq!(POLLFD_REVENTS, 6);
    }

    #[test]
    fn linuxs_events_have_its_numbers() {
        let all = [
            POLLIN, POLLPRI, POLLOUT, POLLERR, POLLHUP, POLLNVAL, POLLRDNORM, POLLRDBAND,
            POLLWRNORM, POLLWRBAND,
        ];
        assert_eq!(all, [1, 2, 4, 8, 0x10, 0x20, 0x40, 0x80, 0x100, 0x200]);
    }

    #[test]
    fn plain_events_need_no_translation() {
        let plain = POLLIN | POLLOUT | POLLPRI | POLLERR | POLLHUP | POLLNVAL;
        assert_eq!(native_events(plain), plain);
    }

    #[test]
    fn events_the_kernel_does_not_know_are_left_out() {
        assert_eq!(native_events(POLLIN | 0x2000), POLLIN);
    }

    #[test]
    fn band_events_stand_for_the_plain_ones() {
        assert_eq!(native_events(POLLRDNORM), POLLIN);
        assert_eq!(native_events(POLLRDBAND), POLLPRI);
        assert_eq!(native_events(POLLWRNORM | POLLWRBAND), POLLOUT);
        // What the kernel does not know is not asked for.
        assert_eq!(native_events(0x2000 | 0x400), 0);
    }

    #[test]
    fn found_events_are_those_asked_for_and_the_always_ones() {
        assert_eq!(found_events(POLLIN, POLLIN | POLLOUT), POLLIN);
        assert_eq!(found_events(POLLIN, POLLHUP), POLLHUP);
        assert_eq!(found_events(0, POLLERR | POLLNVAL), POLLERR | POLLNVAL);
        assert_eq!(found_events(POLLRDNORM, POLLIN), POLLRDNORM);
        assert_eq!(found_events(POLLRDNORM | POLLIN, POLLIN), POLLRDNORM | POLLIN);
        assert_eq!(found_events(POLLWRBAND, POLLOUT), POLLWRBAND);
        assert_eq!(found_events(POLLRDBAND, POLLIN), 0);
    }

    #[test]
    fn descriptor_sets_are_words_of_bits() {
        assert_eq!(set_bytes(0), 0);
        assert_eq!(set_bytes(1), 8);
        assert_eq!(set_bytes(64), 8);
        assert_eq!(set_bytes(65), 16);
        assert_eq!(set_bytes(FD_SETSIZE), 128);
        let mut set = [0u8; 16];
        add_to_set(&mut set, 0);
        add_to_set(&mut set, 9);
        add_to_set(&mut set, 64);
        assert_eq!(set[0], 1);
        assert_eq!(set[1], 2);
        assert_eq!(set[8], 1);
        assert!(in_set(&set, 9) && in_set(&set, 64) && !in_set(&set, 10));
        // Past the set, nothing is in it and nothing is added.
        add_to_set(&mut set, 200);
        assert!(!in_set(&set, 200));
    }

    #[test]
    fn select_sets_become_events_and_back() {
        assert_eq!(select_events(true, false, false), POLLIN);
        assert_eq!(select_events(true, true, true), POLLIN | POLLOUT | POLLPRI);
        assert_eq!(select_events(false, false, false), 0);
        assert_eq!(select_ready(POLLIN, POLLIN), [true, false, false]);
        // An end or an error is something to read, an error something to
        // write.
        assert_eq!(select_ready(POLLIN | POLLOUT, POLLHUP), [true, false, false]);
        assert_eq!(select_ready(POLLIN | POLLOUT, POLLERR), [true, true, false]);
        assert_eq!(select_ready(POLLOUT | POLLPRI, POLLPRI), [false, false, true]);
        assert_eq!(select_ready(POLLIN | POLLOUT | POLLPRI, 0), [false; 3]);
    }

    #[test]
    fn waits_are_in_nanoseconds_and_endless_for_none() {
        assert_eq!(poll_wait(-1), None);
        assert_eq!(poll_wait(i32::MIN), None);
        assert_eq!(poll_wait(0), Some(0));
        assert_eq!(poll_wait(1500), Some(1_500_000_000));
        assert_eq!(wait_for(None), None);
        assert_eq!(wait_for(Some(Time { sec: 2, nsec: 5 })), Some(2_000_000_005));
        assert_eq!(wait_for(Some(Time { sec: i64::MAX / 1_000_000_000, nsec: 0 })), None);
        assert_eq!(time_left(1_500_000_000), Time { sec: 1, nsec: 500_000_000 });
    }

    proptest::proptest! {
        #[test]
        fn only_events_asked_for_or_always_ones_are_found(events: u16, found: u16) {
            let out = found_events(events, found);
            proptest::prop_assert_eq!(out & !(events | POLLERR | POLLHUP | POLLNVAL), 0);
        }
    }
}
