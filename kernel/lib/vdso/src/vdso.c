/*
 * SPDX-FileCopyrightText: 2026 The Dukovo contributors
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 *
 * The vDSO (ADR 0616): the code every program finds mapped, through
 * AT_SYSINFO_EHDR, that reads the clocks without a system call.
 *
 * The clock page (clock_page.h) is the page right below the image, where
 * the linker script puts `dukovo_clock_page`. The kernel writes it under a
 * sequence count, odd while it writes; a reader that saw an odd count, or
 * a count that changed while it read, reads again. The monotonic clock is
 * the counter's ticks since `counter_start` times `ns_per_tick` over 2^32,
 * as the kernel's own; the time of day is that plus the wall clock's
 * offset and as much of its slew as has been applied, as the kernel works
 * it out (radix_hal::clock::Fields::at).
 *
 * Linux programs call the `__vdso_` functions, and the names without the
 * prefix, of version LINUX_2.6, as Linux's: each makes Linux's system call
 * for what it cannot read here (a clock it does not have, or a time of day
 * the kernel does not know yet). Dukovo's C library calls
 * `__dukovo_clock_read`, which never makes a call: it returns -1 for the
 * library to ask the kernel itself.
 *
 * The code must work wherever it is mapped: it is built position
 * independent, with no relocations and nothing written (build.rs checks
 * the image), and calls nothing.
 */

#include <stdint.h>

#include "clock_page.h"

/* The clocks, as Linux and radix-process(2)'s ClockKind number them. */
enum {
	CLOCK_REALTIME = 0,
	CLOCK_MONOTONIC = 1,
	CLOCK_MONOTONIC_RAW = 4,
	CLOCK_REALTIME_COARSE = 5,
	CLOCK_MONOTONIC_COARSE = 6,
	CLOCK_BOOTTIME = 7,
};

/* Linux's system call numbers on x86-64. */
enum {
	SYS_GETTIMEOFDAY = 96,
	SYS_TIME = 201,
	SYS_CLOCK_GETTIME = 228,
	SYS_CLOCK_GETRES = 229,
	SYS_GETCPU = 309,
};

#define NS_PER_SECOND UINT64_C(1000000000)

/* Linux's records. */
struct linux_timespec {
	int64_t tv_sec;
	int64_t tv_nsec;
};

struct linux_timeval {
	int64_t tv_sec;
	int64_t tv_usec;
};

struct linux_timezone {
	int32_t tz_minuteswest;
	int32_t tz_dsttime;
};

extern const struct dukovo_clock_page dukovo_clock_page __attribute__((visibility("hidden")));

#define EXPORT __attribute__((visibility("default")))

/* A field of the page, read once. */
#define FIELD(name) __atomic_load_n(&dukovo_clock_page.name, __ATOMIC_RELAXED)

/* What one reading of the page and the counter saw. */
struct reading {
	/* The monotonic clock. */
	uint64_t now;
	uint64_t wall_known;
	int64_t wall_offset;
	uint64_t wall_slew_start;
	int64_t wall_slew;
};

/* The counter, read after every load before it. */
static inline uint64_t counter(void) {
	uint32_t low, high;
	__asm__ volatile("lfence\n\trdtsc" : "=a"(low), "=d"(high) : : "memory");
	return ((uint64_t)high << 32) | low;
}

/* Reads the page and the clock: 0, or -1 if programs may not read the
 * counter, and must ask the kernel. */
static int read_page(struct reading *out) {
	for(;;) {
		uint64_t before = __atomic_load_n(&dukovo_clock_page.sequence, __ATOMIC_ACQUIRE);
		if(before & 1) {
			__asm__ volatile("pause");
			continue;
		}
		if(!(FIELD(flags) & DUKOVO_CLOCK_COUNTER))
			return -1;
		uint64_t start = FIELD(counter_start);
		uint64_t ns_per_tick = FIELD(ns_per_tick);
		out->wall_known = FIELD(wall_known);
		out->wall_offset = FIELD(wall_offset);
		out->wall_slew_start = FIELD(wall_slew_start);
		out->wall_slew = FIELD(wall_slew);
		uint64_t ticks = counter();
		__atomic_thread_fence(__ATOMIC_ACQUIRE);
		if(__atomic_load_n(&dukovo_clock_page.sequence, __ATOMIC_RELAXED) != before)
			continue;
		out->now = (uint64_t)(((unsigned __int128)(ticks - start) * ns_per_tick) >> 32);
		return 0;
	}
}

/* The time of day a reading says, which knows it. */
static uint64_t wall(const struct reading *r) {
	uint64_t since = r->now > r->wall_slew_start ? r->now - r->wall_slew_start : 0;
	uint64_t most = since / DUKOVO_SLEW_DIVISOR;
	int64_t limit = most > (uint64_t)INT64_MAX ? INT64_MAX : (int64_t)most;
	int64_t applied = r->wall_slew;
	if(applied > limit)
		applied = limit;
	if(applied < -limit)
		applied = -limit;
	__int128 at = (__int128)r->wall_offset + (__int128)r->now + applied;
	if(at < 0)
		return 0;
	if(at > (__int128)UINT64_MAX)
		return UINT64_MAX;
	return (uint64_t)at;
}

/* Whether `clock` is one this file reads. */
static int readable(int clock) {
	switch(clock) {
	case CLOCK_REALTIME:
	case CLOCK_REALTIME_COARSE:
	case CLOCK_MONOTONIC:
	case CLOCK_MONOTONIC_RAW:
	case CLOCK_MONOTONIC_COARSE:
	case CLOCK_BOOTTIME:
		return 1;
	default:
		return 0;
	}
}

/* Clock `clock` in nanoseconds: 0, or -1 if the kernel must be asked. */
static int clock_ns(int clock, uint64_t *ns) {
	struct reading r;
	if(!readable(clock) || read_page(&r))
		return -1;
	if(clock == CLOCK_REALTIME || clock == CLOCK_REALTIME_COARSE) {
		if(!r.wall_known)
			return -1;
		*ns = wall(&r);
	} else {
		*ns = r.now;
	}
	return 0;
}

/* Linux's system call `number`, with three arguments. */
static long linux_call(long number, long first, long second, long third) {
	long result;
	__asm__ volatile("syscall"
			: "=a"(result)
			: "a"(number), "D"(first), "S"(second), "d"(third)
			: "rcx", "r11", "memory");
	return result;
}

/* Dukovo's C library's clock: `clock` as radix-process(2)'s ClockKind
 * numbers it, in nanoseconds at `ns`. */
EXPORT int __dukovo_clock_read(int clock, uint64_t *ns) {
	return clock_ns(clock, ns);
}

EXPORT int __vdso_clock_gettime(int clock, struct linux_timespec *ts) {
	uint64_t ns;
	if(clock_ns(clock, &ns))
		return (int)linux_call(SYS_CLOCK_GETTIME, clock, (long)ts, 0);
	ts->tv_sec = (int64_t)(ns / NS_PER_SECOND);
	ts->tv_nsec = (int64_t)(ns % NS_PER_SECOND);
	return 0;
}

EXPORT int __vdso_gettimeofday(struct linux_timeval *tv, struct linux_timezone *tz) {
	if(tv) {
		uint64_t ns;
		if(clock_ns(CLOCK_REALTIME, &ns))
			return (int)linux_call(SYS_GETTIMEOFDAY, (long)tv, (long)tz, 0);
		tv->tv_sec = (int64_t)(ns / NS_PER_SECOND);
		tv->tv_usec = (int64_t)(ns % NS_PER_SECOND / 1000);
	}
	if(tz) {
		/* As the kernel gives it: UTC, with no daylight saving rule. */
		tz->tz_minuteswest = 0;
		tz->tz_dsttime = 0;
	}
	return 0;
}

EXPORT int64_t __vdso_time(int64_t *t) {
	uint64_t ns;
	if(clock_ns(CLOCK_REALTIME, &ns))
		return linux_call(SYS_TIME, (long)t, 0, 0);
	int64_t seconds = (int64_t)(ns / NS_PER_SECOND);
	if(t)
		*t = seconds;
	return seconds;
}

EXPORT int __vdso_clock_getres(int clock, struct linux_timespec *res) {
	if(!readable(clock))
		return (int)linux_call(SYS_CLOCK_GETRES, clock, (long)res, 0);
	/* The kernel's clocks count nanoseconds. */
	if(res) {
		res->tv_sec = 0;
		res->tv_nsec = 1;
	}
	return 0;
}

EXPORT int __vdso_getcpu(uint32_t *cpu, uint32_t *node, void *cache) {
	uint64_t flags = FIELD(flags);
	uint32_t number;
	if(flags & DUKOVO_CLOCK_CPU_RDPID) {
		uint64_t aux;
		__asm__ volatile("rdpid %0" : "=r"(aux));
		number = (uint32_t)aux;
	} else if(flags & DUKOVO_CLOCK_CPU_RDTSCP) {
		uint32_t low, high, aux;
		__asm__ volatile("rdtscp" : "=a"(low), "=d"(high), "=c"(aux));
		number = aux;
	} else {
		return (int)linux_call(SYS_GETCPU, (long)cpu, (long)node, (long)cache);
	}
	if(cpu)
		*cpu = number;
	/* The kernel has one memory node. */
	if(node)
		*node = 0;
	return 0;
}

/* The names without the prefix, as Linux's vDSO has them. */
EXPORT int clock_gettime(int, struct linux_timespec *)
		__attribute__((weak, alias("__vdso_clock_gettime")));
EXPORT int gettimeofday(struct linux_timeval *, struct linux_timezone *)
		__attribute__((weak, alias("__vdso_gettimeofday")));
EXPORT int64_t time(int64_t *) __attribute__((weak, alias("__vdso_time")));
EXPORT int clock_getres(int, struct linux_timespec *)
		__attribute__((weak, alias("__vdso_clock_getres")));
EXPORT int getcpu(uint32_t *, uint32_t *, void *) __attribute__((weak, alias("__vdso_getcpu")));
