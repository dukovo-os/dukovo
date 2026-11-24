// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The image run on the build host, mapped as the kernel maps it: read
//! and execute, right above a clock page, which here holds what the HAL's
//! writer wrote into a `WallClock`'s page. Each function is found by its
//! name, as a C library finds it, and called. What the image reads is
//! checked against the counter read just before and just after the call,
//! and against the HAL's own arithmetic; what it asks the kernel for, the
//! host's Linux answers.

use core::ffi::c_void;
use core::ptr;
use std::arch::x86_64::{__cpuid, _rdtsc};

use radix_hal::clock::{ClockPage, Counter, WallClock};

use super::IMAGE;

const PAGE: usize = 4096;

unsafe extern "C" {
    fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64)
    -> *mut c_void;
    fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
    fn munmap(addr: *mut c_void, len: usize) -> i32;
}

const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const PROT_EXEC: i32 = 4;
const MAP_PRIVATE: i32 = 2;
const MAP_ANONYMOUS: i32 = 0x20;

type Read = unsafe extern "C" fn(i32, *mut u64) -> i32;
type Timespec = [i64; 2];
type GetTime = unsafe extern "C" fn(i32, *mut Timespec) -> i32;
type GetTimeOfDay = unsafe extern "C" fn(*mut Timespec, *mut [i32; 2]) -> i32;
type Time = unsafe extern "C" fn(*mut i64) -> i64;
type GetCpu = unsafe extern "C" fn(*mut u32, *mut u32, *mut c_void) -> i32;

/// The clock page, then the image, in memory of the test's own.
struct Mapped {
    base: *mut u8,
    len: usize,
}

impl Mapped {
    fn new() -> Mapped {
        assert!(!IMAGE.is_empty());
        let len = PAGE + IMAGE.len().next_multiple_of(PAGE);
        // SAFETY: a new private anonymous mapping, which nothing else uses.
        let base = unsafe {
            mmap(ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0)
        };
        assert!(!base.is_null() && base as isize != -1, "mmap failed");
        let base = base.cast::<u8>();
        // SAFETY: the mapping has room for the image above its first page,
        // and is writable until the protection changes below.
        unsafe {
            ptr::copy_nonoverlapping(IMAGE.as_ptr(), base.add(PAGE), IMAGE.len());
            let image = base.add(PAGE).cast::<c_void>();
            assert_eq!(mprotect(image, len - PAGE, PROT_READ | PROT_EXEC), 0);
        }
        Mapped { base, len }
    }

    /// Makes the clock page hold what `page` holds.
    fn set_page(&self, page: &ClockPage) {
        // SAFETY: both are a page; nothing writes `page` while it is read,
        // and the first page of the mapping is writable.
        unsafe {
            ptr::copy_nonoverlapping(ptr::from_ref(page).cast::<u8>(), self.base, PAGE);
        }
    }

    /// The address of the exported function `name`, found through the
    /// dynamic segment's hash, symbol and string tables.
    fn address(&self, name: &str) -> usize {
        let word = |at: usize| u64::from_le_bytes(IMAGE[at..at + 8].try_into().unwrap());
        let half = |at: usize| u32::from_le_bytes(IMAGE[at..at + 4].try_into().unwrap());
        let (phoff, phnum) = (word(32) as usize, usize::from(IMAGE[56]));
        let dynamic = (0..phnum)
            .map(|index| phoff + index * 56)
            .find(|&at| half(at) == 2)
            .map(|at| word(at + 8) as usize)
            .expect("a dynamic segment");
        let (mut hash, mut strtab, mut symtab) = (0, 0, 0);
        for at in (dynamic..).step_by(16) {
            match word(at) {
                0 => break,
                4 => hash = word(at + 8) as usize,
                5 => strtab = word(at + 8) as usize,
                6 => symtab = word(at + 8) as usize,
                _ => {}
            }
        }
        let count = half(hash + 4) as usize;
        let symbol = (1..count)
            .map(|index| symtab + index * 24)
            .find(|&at| {
                let start = strtab + half(at) as usize;
                let end = start + IMAGE[start..].iter().position(|&b| b == 0).unwrap();
                &IMAGE[start..end] == name.as_bytes()
            })
            .unwrap_or_else(|| panic!("{name} is not exported"));
        self.base as usize + PAGE + word(symbol + 8) as usize
    }

    fn read(&self) -> Read {
        // SAFETY: the address is of the image's function of this type.
        unsafe { core::mem::transmute::<usize, Read>(self.address("__dukovo_clock_read")) }
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        // SAFETY: the mapping `new` made, which nothing refers to now.
        unsafe { munmap(self.base.cast(), self.len) };
    }
}

fn counter() -> u64 {
    // SAFETY: the time-stamp counter can be read in user mode on Linux.
    unsafe { _rdtsc() }
}

/// Reads clock `clock` with `__dukovo_clock_read`, between two reads of
/// the counter: (before, the reading, after), or `None` if it says to ask
/// the kernel.
fn read_between(mapped: &Mapped, clock: i32) -> Option<(u64, u64, u64)> {
    let read = mapped.read();
    let mut ns = 0;
    let before = counter();
    // SAFETY: `ns` is a place for the reading.
    let status = unsafe { read(clock, &raw mut ns) };
    let after = counter();
    (status == 0).then_some((before, ns, after))
}

/// A wall clock whose page says the counter's ticks are nanoseconds,
/// counted from `start`.
fn clock_counting_from(start: u64) -> WallClock {
    let wall = WallClock::new();
    wall.set_counter(Counter { start, ns_per_tick: 1 << 32, flags: ClockPage::COUNTER });
    wall
}

#[test]
fn the_monotonic_clocks_count_from_the_start() {
    let mapped = Mapped::new();
    let start = counter().saturating_sub(1_000_000);
    mapped.set_page(clock_counting_from(start).page());
    for clock in [1, 4, 6, 7] {
        let (before, ns, after) = read_between(&mapped, clock).expect("a reading");
        assert!((before - start..=after - start).contains(&ns), "clock {clock}");
    }
    // A scale of a quarter of a nanosecond a tick.
    let wall = WallClock::new();
    wall.set_counter(Counter { start, ns_per_tick: 1 << 30, flags: ClockPage::COUNTER });
    mapped.set_page(wall.page());
    let (before, ns, after) = read_between(&mapped, 1).expect("a reading");
    assert!(((before - start) / 4..=(after - start) / 4).contains(&ns));
}

#[test]
fn the_time_of_day_is_the_kernels_arithmetic_with_its_slew() {
    let mapped = Mapped::new();
    let start = counter().saturating_sub(10_000_000_000);
    for slew in [0, 3_000_000_000_000, -3_000_000_000_000, 1_000, -1_000] {
        let wall = clock_counting_from(start);
        wall.set(0, 1_700_000_000_000_000_000);
        wall.slew(4_000_000_000, slew).expect("a known time of day");
        mapped.set_page(wall.page());
        for clock in [0, 5] {
            let (before, ns, after) = read_between(&mapped, clock).expect("a reading");
            let earliest = wall.read(before - start).unwrap();
            let latest = wall.read(after - start).unwrap();
            assert!((earliest..=latest).contains(&ns), "slew {slew}, clock {clock}");
        }
    }
    // Before 1970 reads as 1970.
    let wall = clock_counting_from(start);
    wall.set(u64::MAX / 2, 5);
    mapped.set_page(wall.page());
    assert_eq!(read_between(&mapped, 0).map(|(_, ns, _)| ns), Some(0));
}

#[test]
fn what_the_page_cannot_tell_is_asked_of_the_kernel() {
    let mapped = Mapped::new();
    // No time of day yet.
    mapped.set_page(clock_counting_from(0).page());
    assert!(read_between(&mapped, 0).is_none());
    assert!(read_between(&mapped, 1).is_some());
    // Clocks it does not read: CPU time, the alarms, a CPU clock.
    for clock in [2, 3, 8, 9, 11, -2] {
        assert!(read_between(&mapped, clock).is_none(), "clock {clock}");
    }
    // A page that does not name the counter.
    mapped.set_page(WallClock::new().page());
    assert!(read_between(&mapped, 1).is_none());
}

#[test]
fn linuxs_functions_read_the_page_or_make_linuxs_calls() {
    let mapped = Mapped::new();
    // The test takes far fewer than 10^9 ticks: the time of day stays
    // within its first second.
    let start = counter();
    let wall = clock_counting_from(start);
    wall.set(0, 1_800_000_000_000_000_000);
    mapped.set_page(wall.page());
    let (gettime, getres, gettimeofday, time) = (
        mapped.address("__vdso_clock_gettime"),
        mapped.address("__vdso_clock_getres"),
        mapped.address("__vdso_gettimeofday"),
        mapped.address("__vdso_time"),
    );
    // SAFETY: each address is of the image's function of the type.
    let (gettime, getres, gettimeofday, time) = unsafe {
        (
            core::mem::transmute::<usize, GetTime>(gettime),
            core::mem::transmute::<usize, GetTime>(getres),
            core::mem::transmute::<usize, GetTimeOfDay>(gettimeofday),
            core::mem::transmute::<usize, Time>(time),
        )
    };
    // The unprefixed names are the same functions.
    assert_eq!(mapped.address("clock_gettime"), mapped.address("__vdso_clock_gettime"));
    assert_eq!(mapped.address("time"), mapped.address("__vdso_time"));
    let (mut ts, mut tz) = ([0i64; 2], [7i32; 2]);
    let before = counter();
    // SAFETY: each pointer is to a record of the size the call writes.
    unsafe {
        assert_eq!(gettime(1, &raw mut ts), 0);
        let ns = ts[0] as u64 * 1_000_000_000 + ts[1] as u64;
        assert!((before - start..=counter() - start).contains(&ns));
        assert!((0..1_000_000_000).contains(&ts[1]));
        assert_eq!(gettimeofday(&raw mut ts, &raw mut tz), 0);
        assert_eq!(ts[0], 1_800_000_000);
        assert!((0..1_000_000).contains(&ts[1]));
        assert_eq!(tz, [0, 0]);
        assert_eq!(time(ptr::null_mut()), 1_800_000_000);
        let mut seconds = 0;
        assert_eq!(time(&raw mut seconds), 1_800_000_000);
        assert_eq!(seconds, 1_800_000_000);
        assert_eq!(getres(6, &raw mut ts), 0);
        assert_eq!(ts, [0, 1]);
        // The process's CPU time is the host's to give.
        assert_eq!(gettime(2, &raw mut ts), 0);
        assert!(ts[0] >= 0 && (0..1_000_000_000).contains(&ts[1]));
        assert_eq!(getres(2, &raw mut ts), 0);
        // A clock no system has.
        assert_eq!(gettime(1000, &raw mut ts), -22);
    }
    // Without a time of day, the host's is read.
    mapped.set_page(clock_counting_from(start).page());
    // SAFETY: as above.
    unsafe {
        assert_eq!(gettime(0, &raw mut ts), 0);
        assert!(ts[0] > 1_600_000_000);
        assert!(time(ptr::null_mut()) > 1_600_000_000);
    }
}

#[test]
fn getcpu_reads_the_cpus_number_as_the_page_says() {
    let mapped = Mapped::new();
    // SAFETY: the address is of the image's function of this type.
    let getcpu = unsafe { core::mem::transmute::<usize, GetCpu>(mapped.address("__vdso_getcpu")) };
    let (rdtscp, rdpid) =
        (__cpuid(0x8000_0001).edx & (1 << 27) != 0, __cpuid(7).ecx & (1 << 22) != 0);
    let mut flags = vec![0];
    if rdtscp {
        flags.push(ClockPage::CPU_RDTSCP);
    }
    if rdpid {
        flags.push(ClockPage::CPU_RDPID);
    }
    for flag in flags {
        let wall = WallClock::new();
        wall.set_counter(Counter { start: 0, ns_per_tick: 1 << 32, flags: flag });
        mapped.set_page(wall.page());
        let (mut cpu, mut node) = (u32::MAX, u32::MAX);
        // SAFETY: each pointer is to a place for the number it gets.
        assert_eq!(unsafe { getcpu(&raw mut cpu, &raw mut node, ptr::null_mut()) }, 0);
        // The host's Linux keeps its node in the bits above 12.
        let threads = std::thread::available_parallelism().map_or(1, usize::from);
        assert!(((cpu & 0xfff) as usize) < threads.max(4096), "flag {flag}: cpu {cpu}");
        assert!(flag != 0 || node < 64, "node {node}");
        // SAFETY: null pointers ask for nothing.
        assert_eq!(unsafe { getcpu(ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) }, 0);
    }
}
