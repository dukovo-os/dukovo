/*
 * SPDX-FileCopyrightText: 2026 The Dukovo contributors
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 *
 * The clock page as the vDSO reads it (ADR 0613): the layout of
 * radix_hal::clock::ClockPage, which the kernel writes. Both sides check
 * the offsets: the Rust with const assertions, this file with the static
 * ones below.
 */

#ifndef DUKOVO_CLOCK_PAGE_H
#define DUKOVO_CLOCK_PAGE_H

#include <stddef.h>
#include <stdint.h>

/* Programs may read the counter the monotonic clock runs on. */
#define DUKOVO_CLOCK_COUNTER (UINT64_C(1) << 0)
/* A program reads its CPU's number with RDPID. */
#define DUKOVO_CLOCK_CPU_RDPID (UINT64_C(1) << 1)
/* A program reads its CPU's number with RDTSCP. */
#define DUKOVO_CLOCK_CPU_RDTSCP (UINT64_C(1) << 2)

/*
 * How far a slew moves the time of day each second, in nanoseconds
 * (radix_hal::clock::SLEW_RATE_NS): a slew has moved it by at most the
 * time since it started divided by this.
 */
#define DUKOVO_SLEW_DIVISOR (UINT64_C(1000000000) / UINT64_C(500000))

struct dukovo_clock_page {
	/* Odd while the kernel writes the page. */
	uint64_t sequence;
	/* The counter's value when the monotonic clock read 0. */
	uint64_t counter_start;
	/* Nanoseconds per tick of the counter, times 2^32. */
	uint64_t ns_per_tick;
	/* DUKOVO_CLOCK_* */
	uint64_t flags;
	/* Whether the time of day is known: 0 or 1. */
	uint64_t wall_known;
	/* Nanoseconds since 1970 when the monotonic clock read 0, before
	 * the slew. */
	int64_t wall_offset;
	/* When the slew started, on the monotonic clock. */
	uint64_t wall_slew_start;
	/* How far the slew moves the time of day in all. */
	int64_t wall_slew;
};

_Static_assert(offsetof(struct dukovo_clock_page, sequence) == 0, "layout");
_Static_assert(offsetof(struct dukovo_clock_page, counter_start) == 8, "layout");
_Static_assert(offsetof(struct dukovo_clock_page, ns_per_tick) == 16, "layout");
_Static_assert(offsetof(struct dukovo_clock_page, flags) == 24, "layout");
_Static_assert(offsetof(struct dukovo_clock_page, wall_known) == 32, "layout");
_Static_assert(offsetof(struct dukovo_clock_page, wall_offset) == 40, "layout");
_Static_assert(offsetof(struct dukovo_clock_page, wall_slew_start) == 48, "layout");
_Static_assert(offsetof(struct dukovo_clock_page, wall_slew) == 56, "layout");

#endif
