// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's event devices (_/dev/input/eventN_, linux/input.h), as far
//! as the Linux side of the input benchmarks uses them
//! (`tools/linux-bench`): a device's name, the clock its events are
//! stamped with, and an event as read(2) gives it.

use std::io;
use std::os::fd::{AsFd, AsRawFd};

use crate::checked;

/// The longest name [`name`] reads.
const NAME_MAX: usize = 64;

/// EVIOCGNAME of [`NAME_MAX`] bytes, and EVIOCSCLOCKID.
const EVIOCGNAME: libc::c_ulong = 0x8000_4506 | ((NAME_MAX as libc::c_ulong) << 16);
const EVIOCSCLOCKID: libc::c_ulong = 0x4004_45a0;

/// The bytes of one event as read(2) gives it on a 64-bit machine.
pub const EVENT_LEN: usize = 24;

/// EV_SYN, the kind of the event that ends a report.
pub const EV_SYN: u16 = 0;
/// SYN_REPORT, that event's code.
pub const SYN_REPORT: u16 = 0;

/// One event of a device (input_event).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    /// When the kernel took it from the device's driver, in nanoseconds
    /// of the clock the device stamps with; the kernel gives
    /// microseconds.
    pub time_ns: u64,
    /// What kind: an EV_*.
    pub kind: u16,
    /// Which key or axis.
    pub code: u16,
    /// What it did.
    pub value: i32,
}

impl Event {
    /// The event that `bytes` hold.
    #[must_use]
    pub fn of(bytes: &[u8; EVENT_LEN]) -> Event {
        let number = |at: usize| {
            let mut eight = [0u8; 8];
            eight.copy_from_slice(&bytes[at..at + 8]);
            u64::from_ne_bytes(eight)
        };
        Event {
            time_ns: number(0)
                .wrapping_mul(1_000_000_000)
                .wrapping_add(number(8).wrapping_mul(1000)),
            kind: u16::from_ne_bytes([bytes[16], bytes[17]]),
            code: u16::from_ne_bytes([bytes[18], bytes[19]]),
            value: i32::from_ne_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
        }
    }
}

/// The name the device `device` gives itself.
///
/// # Errors
///
/// ENOTTY for a file that is no event device.
pub fn name(device: impl AsFd) -> io::Result<String> {
    let mut name = [0u8; NAME_MAX];
    // SAFETY: the request writes at most the bytes its number gives,
    // which the array has.
    let len =
        checked(unsafe { libc::ioctl(device.as_fd().as_raw_fd(), EVIOCGNAME, name.as_mut_ptr()) })?;
    let len = usize::try_from(len).unwrap_or(0).min(NAME_MAX);
    let len = name[..len].iter().position(|&byte| byte == 0).unwrap_or(len);
    Ok(String::from_utf8_lossy(&name[..len]).into_owned())
}

/// Has this description of `device` stamp its events with the monotonic
/// clock, which [`crate::time::monotonic_ns`] reads, in place of the
/// wall clock.
///
/// # Errors
///
/// ENOTTY for a file that is no event device.
pub fn stamp_with_monotonic(device: impl AsFd) -> io::Result<()> {
    let clock: libc::c_int = libc::CLOCK_MONOTONIC;
    // SAFETY: the request reads an int, which lives through the call.
    checked(unsafe { libc::ioctl(device.as_fd().as_raw_fd(), EVIOCSCLOCKID, &raw const clock) })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_is_read_from_its_bytes() {
        let mut bytes = [0u8; EVENT_LEN];
        bytes[..8].copy_from_slice(&3u64.to_ne_bytes());
        bytes[8..16].copy_from_slice(&250_000u64.to_ne_bytes());
        bytes[16..18].copy_from_slice(&2u16.to_ne_bytes());
        bytes[18..20].copy_from_slice(&1u16.to_ne_bytes());
        bytes[20..].copy_from_slice(&(-5i32).to_ne_bytes());
        let event = Event::of(&bytes);
        assert_eq!(event, Event { time_ns: 3_250_000_000, kind: 2, code: 1, value: -5 });
    }

    #[test]
    fn a_file_that_is_no_event_device_is_refused() {
        let null = std::fs::File::open("/dev/null").unwrap();
        assert!(name(&null).is_err());
        assert!(stamp_with_monotonic(&null).is_err());
    }
}
