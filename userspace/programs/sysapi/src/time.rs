// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Time on the calendar in the system's time zone, as the C library keeps
//! it (localtime_r(3), mktime(3) and strftime(3), which read TZ and the
//! zone files), and the monotonic clock in nanoseconds.

use std::ffi::CString;
use std::io;

use crate::checked;

/// A date and time on the calendar, as the C library's `struct tm` holds
/// one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Civil {
    /// The year, 2026.
    pub year: i64,
    /// The month, 1 to 12.
    pub month: u32,
    /// The day of the month, 1 to 31.
    pub day: u32,
    /// The hour, 0 to 23.
    pub hour: u32,
    /// The minute, 0 to 59.
    pub minute: u32,
    /// The second, 0 to 60 for a leap second.
    pub second: u32,
    /// The day of the week, 0 for Sunday to 6.
    pub weekday: u32,
    /// The day of the year, 0 for January 1st to 365.
    pub yearday: u32,
}

impl Civil {
    fn of(tm: &libc::tm) -> Civil {
        // Every field the C library fills is in its range, so each fits.
        let field = |value: libc::c_int| u32::try_from(value).unwrap_or(0);
        Civil {
            year: i64::from(tm.tm_year) + 1900,
            month: field(tm.tm_mon + 1),
            day: field(tm.tm_mday),
            hour: field(tm.tm_hour),
            minute: field(tm.tm_min),
            second: field(tm.tm_sec),
            weekday: field(tm.tm_wday),
            yearday: field(tm.tm_yday),
        }
    }
}

/// The time `seconds` after 1970 UTC, broken down in local time, or in
/// UTC with `utc`, into a `tm`.
fn broken_down(seconds: i64, utc: bool) -> io::Result<libc::tm> {
    let time = libc::time_t::try_from(seconds)
        .map_err(|_| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
    // SAFETY: all zeros is a valid tm: numbers, and a null zone name.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call, which fills `tm`.
    let filled = unsafe {
        if utc {
            libc::gmtime_r(&raw const time, &raw mut tm)
        } else {
            libc::localtime_r(&raw const time, &raw mut tm)
        }
    };
    if filled.is_null() {
        return Err(io::Error::from_raw_os_error(libc::EOVERFLOW));
    }
    Ok(tm)
}

/// The date and time `seconds` after 1970 UTC is in the system's time
/// zone.
///
/// # Errors
///
/// EOVERFLOW for a time whose year does not fit.
pub fn local(seconds: i64) -> io::Result<Civil> {
    broken_down(seconds, false).map(|tm| Civil::of(&tm))
}

/// The date and time `seconds` after 1970 is in UTC.
///
/// # Errors
///
/// EOVERFLOW for a time whose year does not fit.
pub fn utc(seconds: i64) -> io::Result<Civil> {
    broken_down(seconds, true).map(|tm| Civil::of(&tm))
}

/// The seconds after 1970 UTC of the local date and time `civil` (its
/// week and year days are not read), with daylight saving time as the
/// zone has it then, as mktime(3) finds them; and the date and time
/// those seconds are, which differ from `civil` where a field was out of
/// its range or the time fell in a gap the clocks skipped.
///
/// # Errors
///
/// EOVERFLOW for a date the C library cannot place.
pub fn from_local(civil: &Civil) -> io::Result<(i64, Civil)> {
    let field = |value: u32| {
        libc::c_int::try_from(value).map_err(|_| io::Error::from_raw_os_error(libc::EOVERFLOW))
    };
    let year = civil
        .year
        .checked_sub(1900)
        .and_then(|year| libc::c_int::try_from(year).ok())
        .ok_or_else(|| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
    // SAFETY: all zeros is a valid tm: numbers, and a null zone name.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year;
    tm.tm_mon = field(civil.month)? - 1;
    tm.tm_mday = field(civil.day)?;
    tm.tm_hour = field(civil.hour)?;
    tm.tm_min = field(civil.minute)?;
    tm.tm_sec = field(civil.second)?;
    // Whether daylight saving time is in effect then, mktime finds.
    tm.tm_isdst = -1;
    // SAFETY: mktime reads the fields of `tm` and writes them normalized.
    let seconds = unsafe { libc::mktime(&raw mut tm) };
    if seconds == -1 {
        return Err(io::Error::from_raw_os_error(libc::EOVERFLOW));
    }
    // time_t is i64 on every system this builds for.
    #[allow(clippy::useless_conversion)]
    Ok((i64::from(seconds), Civil::of(&tm)))
}

/// The time `seconds` after 1970 UTC written as strftime(3) writes it
/// with `format`, in local time or in UTC; empty for a text longer than
/// 16 KiB.
///
/// # Errors
///
/// EOVERFLOW for a time whose year does not fit; `ErrorKind::InvalidInput`
/// for a format with a NUL byte.
pub fn format(seconds: i64, format: &str, utc: bool) -> io::Result<String> {
    let tm = broken_down(seconds, utc)?;
    let format = crate::c_string(format.as_bytes())?;
    Ok(format_tm(&tm, &format))
}

fn format_tm(tm: &libc::tm, format: &CString) -> String {
    // strftime gives 0 both for a text that does not fit and for an empty
    // one, so a larger buffer is tried up to a limit.
    let mut buffer = vec![0u8; 256];
    loop {
        // SAFETY: strftime writes at most the buffer's length into it, from
        // a C string format and a tm that gmtime_r or localtime_r filled,
        // whose zone name is the C library's own.
        let length = unsafe {
            libc::strftime(buffer.as_mut_ptr().cast(), buffer.len(), format.as_ptr(), tm)
        };
        if length > 0 || buffer.len() >= FORMATTED_MAX {
            return String::from_utf8_lossy(&buffer[..length]).into_owned();
        }
        buffer.resize(buffer.len() * 4, 0);
    }
}

/// The longest time [`format`] writes: a longer one is written as empty.
const FORMATTED_MAX: usize = 16 << 10;

/// The monotonic clock (CLOCK_MONOTONIC) in nanoseconds: the time since
/// the system started, which no change to the wall clock moves, as the
/// kernel's own records count it.
#[must_use]
pub fn monotonic_ns() -> u64 {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime fills the timespec it is given; the monotonic
    // clock is always there.
    let _ = checked(unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut now) });
    let seconds = u64::try_from(now.tv_sec).unwrap_or(0);
    let nanoseconds = u64::try_from(now.tv_nsec).unwrap_or(0);
    seconds * 1_000_000_000 + nanoseconds
}

#[cfg(test)]
mod tests {
    use super::{Civil, format, from_local, monotonic_ns, utc};

    #[test]
    fn a_time_is_broken_down_in_utc() {
        // 2026-11-25 03:00:00 UTC, a Wednesday, the 329th day.
        let civil = utc(1_795_575_600).unwrap();
        let expected = Civil {
            year: 2026,
            month: 11,
            day: 25,
            hour: 3,
            minute: 0,
            second: 0,
            weekday: 3,
            yearday: 328,
        };
        assert_eq!(civil, expected);
        assert_eq!(
            format(1_795_575_600, "%Y-%m-%d %H:%M %a", true).unwrap(),
            "2026-11-25 03:00 Wed"
        );
    }

    #[test]
    fn local_time_goes_there_and_back() {
        let now = 1_795_575_600;
        let civil = super::local(now).unwrap();
        let (seconds, again) = from_local(&civil).unwrap();
        assert_eq!(seconds, now);
        assert_eq!(again, civil);
    }

    #[test]
    fn a_field_out_of_range_comes_back_changed() {
        let civil = Civil { year: 2026, month: 2, day: 30, hour: 12, ..Civil::default() };
        let (_, normalized) = from_local(&civil).unwrap();
        assert_eq!((normalized.month, normalized.day), (3, 2));
        assert!(from_local(&Civil { month: 0, ..civil }).is_ok(), "month 0 is December before");
    }

    #[test]
    fn a_format_with_a_nul_is_refused() {
        assert_eq!(format(0, "%Y\0", true).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn the_monotonic_clock_goes_forward() {
        let before = monotonic_ns();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(monotonic_ns() >= before + 2_000_000);
    }
}
