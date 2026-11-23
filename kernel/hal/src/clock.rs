// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Calendar arithmetic for the wall clock, the time of day kept on the
//! kernel's monotonic clock ([`WallClock`]), the page programs read both
//! clocks from themselves ([`ClockPage`]), and the fixed-point scale that
//! turns a counter's ticks into nanoseconds.

use core::hint;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering, fence};

/// How far a slew moves the time of day each second, in nanoseconds:
/// 500 µs, as Linux's adjtime(3) (radix-clock(2)'s SLEW_RATE_NS).
pub const SLEW_RATE_NS: u64 = 500_000;

/// The time of day, as the kernel keeps it (ADR 0194): an offset from the
/// kernel's monotonic clock, set from the real-time clock at boot and by
/// [`WallClock::set`], and a slew, which runs it faster or slower by
/// [`SLEW_RATE_NS`] a second until it has moved as far as asked, so that
/// small corrections neither jump nor run it backwards.
///
/// Readers never wait for writers, as they read it on every
/// `clock_gettime`: a sequence count, odd while a writer changes the
/// fields, sends a reader that overlapped one round again (a seqlock).
/// Writers are rare, timed(8) every few minutes, and take turns through
/// `writing`.
pub struct WallClock {
    sequence: AtomicU64,
    writing: AtomicBool,
    /// Whether the time of day is known at all.
    known: AtomicBool,
    /// Nanoseconds since 1970 when the monotonic clock read 0, before the
    /// slew.
    offset: AtomicI64,
    /// When the slew started, on the monotonic clock.
    slew_start: AtomicU64,
    /// How far the slew moves the time of day in all, in nanoseconds.
    slew: AtomicI64,
    /// The page programs read it from, which the writer that changes the
    /// fields copies them to.
    page: ClockPage,
}

/// A wall clock's fields, as one reading of them saw them: what a program
/// that reads the time of day from the [`ClockPage`] works it out from,
/// as [`WallClock::read`] does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fields {
    /// Whether the time of day is known at all.
    pub known: bool,
    /// Nanoseconds since 1970 when the monotonic clock read 0, before the
    /// slew.
    pub offset: i64,
    /// When the slew started, on the monotonic clock.
    pub slew_start: u64,
    /// How far the slew moves the time of day in all, in nanoseconds.
    pub slew: i64,
}

impl Fields {
    /// How much of the slew has been applied at `now`.
    fn applied(self, now: u64) -> i64 {
        let most = now.saturating_sub(self.slew_start) / (1_000_000_000 / SLEW_RATE_NS);
        let most = i64::try_from(most).unwrap_or(i64::MAX);
        self.slew.clamp(-most, most)
    }

    /// The time of day when the monotonic clock reads `now`, if it is
    /// known: [`WallClock::read`]'s arithmetic, which the clock page's
    /// readers repeat.
    pub fn at(self, now: u64) -> Option<u64> {
        if !self.known {
            return None;
        }
        let at = i128::from(self.offset) + i128::from(now) + i128::from(self.applied(now));
        Some(u64::try_from(at.max(0)).unwrap_or(u64::MAX))
    }
}

impl WallClock {
    /// A clock that knows no time of day.
    pub const fn new() -> WallClock {
        WallClock {
            sequence: AtomicU64::new(0),
            writing: AtomicBool::new(false),
            known: AtomicBool::new(false),
            offset: AtomicI64::new(0),
            slew_start: AtomicU64::new(0),
            slew: AtomicI64::new(0),
            page: ClockPage::new(),
        }
    }

    /// Its fields, as one reading saw them.
    pub fn fields(&self) -> Fields {
        loop {
            if let Some(fields) = self.fields_once() {
                return fields;
            }
            hint::spin_loop();
        }
    }

    /// The fields, if no change was under way while they were read.
    fn fields_once(&self) -> Option<Fields> {
        let before = self.sequence.load(Ordering::Acquire);
        if before & 1 != 0 {
            return None;
        }
        let fields = Fields {
            known: self.known.load(Ordering::Relaxed),
            offset: self.offset.load(Ordering::Relaxed),
            slew_start: self.slew_start.load(Ordering::Relaxed),
            slew: self.slew.load(Ordering::Relaxed),
        };
        fence(Ordering::Acquire);
        (self.sequence.load(Ordering::Relaxed) == before).then_some(fields)
    }

    /// Runs `write` as the only writer: writers take turns.
    fn as_writer<T>(&self, write: impl FnOnce() -> T) -> T {
        while self
            .writing
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            hint::spin_loop();
        }
        let result = write();
        self.writing.store(false, Ordering::Release);
        result
    }

    /// Changes the fields with `change`, which gets them as they are, and
    /// copies them to its clock page while no other writer runs: the page
    /// always holds what the clock's last change made.
    fn write(&self, change: impl FnOnce(Fields) -> Fields) {
        self.as_writer(|| {
            let new = change(self.fields());
            self.sequence.fetch_add(1, Ordering::Relaxed);
            fence(Ordering::Release);
            self.known.store(new.known, Ordering::Relaxed);
            self.offset.store(new.offset, Ordering::Relaxed);
            self.slew_start.store(new.slew_start, Ordering::Relaxed);
            self.slew.store(new.slew, Ordering::Relaxed);
            self.sequence.fetch_add(1, Ordering::Release);
            self.page.write(|page| page.set_wall(new));
        });
    }

    /// The page programs read it from, with the counter the monotonic
    /// clock runs on: one that tells them to ask the kernel until
    /// [`WallClock::set_counter`] names the counter.
    pub fn page(&self) -> &ClockPage {
        &self.page
    }

    /// Writes `counter`, the counter the monotonic clock runs on, as
    /// programs may read it, into its page. Called when the monotonic
    /// clock has its scale.
    pub fn set_counter(&self, counter: Counter) {
        self.as_writer(|| self.page.write(|page| page.set_counter(counter)));
    }

    /// The time of day when the monotonic clock reads `now`: nanoseconds
    /// since 1970, if it is known.
    pub fn read(&self, now: u64) -> Option<u64> {
        self.fields().at(now)
    }

    /// [`WallClock::read`], but `None` if a change stays half made for a
    /// thousand tries, as one that a CPU stopped in the middle of does.
    pub fn read_without_waiting(&self, now: u64) -> Option<u64> {
        let fields = (0..1000).find_map(|_| {
            let fields = self.fields_once();
            if fields.is_none() {
                hint::spin_loop();
            }
            fields
        })?;
        fields.at(now)
    }

    /// Sets the time of day to `ns` when the monotonic clock reads `now`,
    /// ending any slew.
    pub fn set(&self, now: u64, ns: u64) {
        let offset = i128::from(ns) - i128::from(now);
        let offset = i64::try_from(offset).unwrap_or(if offset < 0 { i64::MIN } else { i64::MAX });
        self.write(|_| Fields { known: true, offset, slew_start: now, slew: 0 });
    }

    /// Starts a slew of `delta` nanoseconds at `now`, in place of any
    /// under way: returns what was left of that one, or `None`, changing
    /// nothing, while the time of day is not known.
    pub fn slew(&self, now: u64, delta: i64) -> Option<i64> {
        let mut left = None;
        self.write(|fields| {
            if !fields.known {
                return fields;
            }
            let applied = fields.applied(now);
            left = Some(fields.slew - applied);
            Fields {
                known: true,
                offset: fields.offset.saturating_add(applied),
                slew_start: now,
                slew: delta,
            }
        });
        left
    }

    /// The slew still to come at `now`, in nanoseconds.
    pub fn slew_left(&self, now: u64) -> i64 {
        let fields = self.fields();
        fields.slew - fields.applied(now)
    }
}

impl Default for WallClock {
    fn default() -> WallClock {
        WallClock::new()
    }
}

/// The counter the kernel's monotonic clock runs on, as programs may read
/// it themselves: the clock reads `ticks_to_ns(counter - start,
/// ns_per_tick)` ([`ticks_to_ns`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counter {
    /// The counter's value when the clock read 0.
    pub start: u64,
    /// Its scale: nanoseconds per tick, times 2^32.
    pub ns_per_tick: u64,
    /// The page's flags: [`ClockPage::COUNTER`] if programs may read the
    /// counter (none ask the kernel for the time), and how they find the
    /// number of the CPU they run on.
    pub flags: u64,
}

/// The clock page (ADR 0613): one page the kernel keeps and every program
/// maps read-only, from which its vDSO reads the monotonic clock and the
/// time of day without a system call. It holds the counter the monotonic
/// clock runs on ([`Counter`]) and the wall clock's fields ([`Fields`]),
/// from which a reader works the time out as the kernel does.
///
/// Readers never wait for the writer, as [`WallClock`]'s don't: a
/// sequence count, odd while the page is written, sends a reader that
/// overlapped a write round again (`kernel/radix/tla/ClockPage.tla`). The
/// page is a [`WallClock`]'s, whose writers take turns and write it.
///
/// Its layout is fixed: the vDSO's C reads it at these offsets
/// (`kernel/lib/vdso/src/clock_page.h`), which both sides check.
#[repr(C, align(4096))]
pub struct ClockPage {
    sequence: AtomicU64,
    counter_start: AtomicU64,
    ns_per_tick: AtomicU64,
    flags: AtomicU64,
    wall_known: AtomicU64,
    wall_offset: AtomicI64,
    wall_slew_start: AtomicU64,
    wall_slew: AtomicI64,
}

// The offsets the vDSO reads the fields at.
const _: () = {
    use core::mem::offset_of;
    assert!(offset_of!(ClockPage, sequence) == 0);
    assert!(offset_of!(ClockPage, counter_start) == 8);
    assert!(offset_of!(ClockPage, ns_per_tick) == 16);
    assert!(offset_of!(ClockPage, flags) == 24);
    assert!(offset_of!(ClockPage, wall_known) == 32);
    assert!(offset_of!(ClockPage, wall_offset) == 40);
    assert!(offset_of!(ClockPage, wall_slew_start) == 48);
    assert!(offset_of!(ClockPage, wall_slew) == 56);
    assert!(size_of::<ClockPage>() == 4096);
};

impl Default for ClockPage {
    fn default() -> ClockPage {
        ClockPage::new()
    }
}

/// What one reading of a [`ClockPage`] saw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    /// The counter, and the page's flags.
    pub counter: Counter,
    /// The wall clock's fields.
    pub wall: Fields,
}

impl ClockPage {
    /// The flag that says programs may read the counter the monotonic
    /// clock runs on: without it, they ask the kernel for the time.
    pub const COUNTER: u64 = 1 << 0;
    /// The flag that says a program reads the number of the CPU it runs
    /// on with RDPID (x86-64), from IA32_TSC_AUX.
    pub const CPU_RDPID: u64 = 1 << 1;
    /// The flag that says a program reads the number of the CPU it runs
    /// on with RDTSCP (x86-64), from IA32_TSC_AUX.
    pub const CPU_RDTSCP: u64 = 1 << 2;

    /// A page that tells programs to ask the kernel.
    pub const fn new() -> ClockPage {
        ClockPage {
            sequence: AtomicU64::new(0),
            counter_start: AtomicU64::new(0),
            ns_per_tick: AtomicU64::new(0),
            flags: AtomicU64::new(0),
            wall_known: AtomicU64::new(0),
            wall_offset: AtomicI64::new(0),
            wall_slew_start: AtomicU64::new(0),
            wall_slew: AtomicI64::new(0),
        }
    }

    /// Runs `change` as a write of the page: the sequence count is odd
    /// while it runs. Its caller is the only writer.
    fn write(&self, change: impl FnOnce(&ClockPage)) {
        self.sequence.fetch_add(1, Ordering::Relaxed);
        fence(Ordering::Release);
        change(self);
        self.sequence.fetch_add(1, Ordering::Release);
    }

    fn set_counter(&self, counter: Counter) {
        self.counter_start.store(counter.start, Ordering::Relaxed);
        self.ns_per_tick.store(counter.ns_per_tick, Ordering::Relaxed);
        self.flags.store(counter.flags, Ordering::Relaxed);
    }

    fn set_wall(&self, fields: Fields) {
        self.wall_known.store(u64::from(fields.known), Ordering::Relaxed);
        self.wall_offset.store(fields.offset, Ordering::Relaxed);
        self.wall_slew_start.store(fields.slew_start, Ordering::Relaxed);
        self.wall_slew.store(fields.slew, Ordering::Relaxed);
    }

    /// Reads the page as a program does: the fields one write left, never
    /// some of one and some of another.
    pub fn read(&self) -> Reading {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 == 0 {
                let reading = Reading {
                    counter: Counter {
                        start: self.counter_start.load(Ordering::Relaxed),
                        ns_per_tick: self.ns_per_tick.load(Ordering::Relaxed),
                        flags: self.flags.load(Ordering::Relaxed),
                    },
                    wall: Fields {
                        known: self.wall_known.load(Ordering::Relaxed) != 0,
                        offset: self.wall_offset.load(Ordering::Relaxed),
                        slew_start: self.wall_slew_start.load(Ordering::Relaxed),
                        slew: self.wall_slew.load(Ordering::Relaxed),
                    },
                };
                fence(Ordering::Acquire);
                if self.sequence.load(Ordering::Relaxed) == before {
                    return reading;
                }
            }
            hint::spin_loop();
        }
    }
}

/// Days from 1970-01-01 to `year`-`month`-`day` in the proleptic Gregorian
/// calendar (negative before 1970), by Howard Hinnant's `days_from_civil`
/// algorithm (public domain).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date of the day `days` after 1970-01-01: (year, month, day).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Days in `month` (1 to 12) of `year`.
fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if year.rem_euclid(4) == 0
            && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0) =>
        {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Seconds since 1970-01-01 00:00:00 UTC of a calendar date and time, if
/// it is valid and not before 1970.
pub fn unix_seconds(
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Option<u64> {
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + i64::from(hour * 3600 + minute * 60 + second);
    u64::try_from(seconds).ok()
}

/// The scale [`ticks_to_ns`] takes for a counter running at `hz`:
/// nanoseconds per tick times 2^32, rounded down.
pub fn ns_per_tick(hz: u64) -> u64 {
    ((1_000_000_000u128 << 32) / u128::from(hz.max(1))) as u64
}

/// Nanoseconds in `ticks` of a counter whose scale is `ns_per_tick`.
pub fn ticks_to_ns(ticks: u64, ns_per_tick: u64) -> u64 {
    ((u128::from(ticks) * u128::from(ns_per_tick)) >> 32) as u64
}

/// The first tick count at which [`ticks_to_ns`] reaches `ns`, so that a
/// timer armed for it never fires before the clock reads `ns`. Scaling
/// `ns` by the counter's rate instead lands early, more so the larger `ns`
/// is, because the scale is rounded down.
pub fn first_tick_at(ns: u64, ns_per_tick: u64) -> u64 {
    let ticks = (u128::from(ns) << 32).div_ceil(u128::from(ns_per_tick.max(1)));
    u64::try_from(ticks).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_clock_set_and_read() {
        let clock = WallClock::new();
        assert_eq!(clock.read(5), None);
        assert_eq!(clock.slew(5, 1000), None);
        clock.set(1_000, 1_700_000_000_000_000_000);
        assert_eq!(clock.read(1_000), Some(1_700_000_000_000_000_000));
        assert_eq!(clock.read(3_000), Some(1_700_000_000_000_002_000));
        // Before 1970 reads as 1970.
        clock.set(10_000_000_000, 5);
        assert_eq!(clock.read(0), Some(0));
    }

    #[test]
    fn the_panic_paths_read_does_not_wait_for_a_change_left_half_made() {
        let clock = WallClock::new();
        assert_eq!(clock.read_without_waiting(5), None);
        clock.set(1_000, 1_700_000_000_000_000_000);
        assert_eq!(clock.read_without_waiting(3_000), Some(1_700_000_000_000_002_000));
        // A CPU stopped in the middle of a change: the count stays odd.
        clock.sequence.fetch_add(1, Ordering::AcqRel);
        assert_eq!(clock.read_without_waiting(3_000), None);
    }

    #[test]
    fn the_clock_page_mirrors_the_wall_clock() {
        let clock = WallClock::new();
        assert_eq!(clock.page().read(), Reading::default());
        clock.set(1_000, 1_700_000_000_000_000_000);
        assert_eq!(clock.page().read().wall, clock.fields());
        let counter = Counter { start: 77, ns_per_tick: 3 << 31, flags: ClockPage::COUNTER };
        clock.set_counter(counter);
        assert_eq!(clock.slew(2_000, -4_000), Some(0));
        let reading = clock.page().read();
        assert_eq!(reading.counter, counter);
        assert_eq!(reading.wall, clock.fields());
        for now in [0, 2_000, 3_000, 1_000_000_000, 9_000_000_000] {
            assert_eq!(reading.wall.at(now), clock.read(now));
        }
        assert_eq!(clock.page().sequence.load(Ordering::Relaxed) % 2, 0);
    }

    #[test]
    fn a_reader_of_the_clock_page_never_sees_half_a_write() {
        static CLOCK: WallClock = WallClock::new();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                // Each write's fields are one number, which no two share.
                for n in 1..20_000u64 {
                    CLOCK.set(n, n * 2);
                }
            });
            for _ in 0..2 {
                scope.spawn(|| {
                    for _ in 0..20_000 {
                        let wall = CLOCK.page().read().wall;
                        if wall.known {
                            assert_eq!(wall.offset.cast_unsigned(), wall.slew_start, "{wall:?}");
                            assert_eq!(wall.slew, 0);
                        }
                    }
                });
            }
        });
    }

    #[test]
    fn wall_clock_slews_at_its_rate() {
        let clock = WallClock::new();
        let second = 1_000_000_000;
        clock.set(0, 100 * second);
        // 1 ms forward takes two seconds.
        assert_eq!(clock.slew(0, 1_000_000), Some(0));
        assert_eq!(clock.read(second), Some(101 * second + 500_000));
        assert_eq!(clock.slew_left(second), 500_000);
        assert_eq!(clock.read(2 * second), Some(102 * second + 1_000_000));
        assert_eq!(clock.read(10 * second), Some(110 * second + 1_000_000));
        assert_eq!(clock.slew_left(10 * second), 0);
        // Backwards, it still never runs back.
        assert_eq!(clock.slew(10 * second, -1_000_000), Some(0));
        let mut last = 0;
        for tenth in 100..150 {
            let now = clock.read(tenth * second / 10).unwrap();
            assert!(now > last);
            last = now;
        }
        assert_eq!(clock.read(20 * second), Some(120 * second));
        // A new slew takes the place of the one under way, and says what
        // was left of it; setting ends it.
        assert_eq!(clock.slew(20 * second, 2_000_000), Some(0));
        assert_eq!(clock.slew(21 * second, 0), Some(1_500_000));
        assert_eq!(clock.read(30 * second), Some(130 * second + 500_000));
        clock.slew(30 * second, 4_000);
        clock.set(30 * second, 7);
        assert_eq!(clock.slew_left(30 * second), 0);
        assert_eq!(clock.read(31 * second), Some(second + 7));
    }

    #[test]
    fn known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(unix_seconds(2026, 10, 22, 0, 17, 56), Some(1_792_628_276));
        assert_eq!(unix_seconds(1969, 12, 31, 23, 59, 59), None);
        assert_eq!(unix_seconds(2026, 13, 1, 0, 0, 0), None);
    }

    #[test]
    fn days_past_the_end_of_the_month_are_refused() {
        assert_eq!(unix_seconds(2024, 2, 29, 0, 0, 0), Some(1_709_164_800));
        assert_eq!(unix_seconds(2000, 2, 29, 0, 0, 0), Some(951_782_400));
        assert_eq!(unix_seconds(2025, 2, 29, 0, 0, 0), None);
        assert_eq!(unix_seconds(2100, 2, 29, 0, 0, 0), None);
        assert_eq!(unix_seconds(2026, 4, 31, 0, 0, 0), None);
        assert_eq!(unix_seconds(2026, 12, 31, 23, 59, 59), Some(1_798_761_599));
        assert_eq!(unix_seconds(2026, 1, 0, 0, 0, 0), None);
        for month in 1..=12 {
            let last = days_in_month(2026, month);
            let next = if month == 12 { (2027, 1) } else { (2026, month + 1) };
            assert_eq!(days_from_civil(2026, month, last) + 1, days_from_civil(next.0, next.1, 1));
        }
    }

    #[test]
    fn a_timer_at_the_first_tick_never_fires_early() {
        for hz in [
            1_000_000,
            14_318_180,
            1_000_000_000,
            2_419_200_000,
            2_995_200_000,
            3_000_000_000,
            4_294_967_311,
        ] {
            let scale = ns_per_tick(hz);
            for ns in [0, 1, 999, 1_000_000_007, 3 * 86_400 * 1_000_000_000 + 12_345, u64::MAX >> 8]
            {
                let tick = first_tick_at(ns, scale);
                assert!(ticks_to_ns(tick, scale) >= ns, "{hz} Hz, {ns} ns");
                assert!(tick == 0 || ticks_to_ns(tick - 1, scale) < ns, "{hz} Hz, {ns} ns");
            }
        }
        // Scaling by the rate instead fires 60 us early a day after boot.
        let (hz, ns) = (2_995_200_001u64, 86_400 * 1_000_000_000u64);
        let scaled = (u128::from(ns) * u128::from(hz) / 1_000_000_000) as u64;
        assert!(ticks_to_ns(scaled, ns_per_tick(hz)) < ns);
    }

    #[test]
    fn round_trip() {
        for days in (-800_000..800_000).step_by(37) {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }
}
