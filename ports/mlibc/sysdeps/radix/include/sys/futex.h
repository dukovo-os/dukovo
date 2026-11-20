/* SPDX-FileCopyrightText: 2026 The Dukovo contributors */
/* SPDX-License-Identifier: Apache-2.0 OR MIT */

/* futex(2) as OpenBSD declares it (ADR 0350): a wait on a 32-bit word
 * while it holds a value, and a wake of the threads waiting on it, for
 * programs that keep their own locks, such as Go's runtime. Every futex
 * is private to its process, so FUTEX_PRIVATE_FLAG changes nothing. */

#ifndef _SYS_FUTEX_H
#define _SYS_FUTEX_H

#include <stdint.h>
#include <time.h>

#ifdef __cplusplus
extern "C" {
#endif

#define FUTEX_WAIT 1
#define FUTEX_WAKE 2
#define FUTEX_PRIVATE_FLAG 128
#define FUTEX_WAIT_PRIVATE (FUTEX_WAIT | FUTEX_PRIVATE_FLAG)
#define FUTEX_WAKE_PRIVATE (FUTEX_WAKE | FUTEX_PRIVATE_FLAG)

/* FUTEX_WAIT: sleeps while *uaddr holds val, until a FUTEX_WAKE on uaddr
 * or for at most *timeout (a relative time; none if null), and returns
 * 0; or -1 with errno EAGAIN (*uaddr held another value), ETIMEDOUT or
 * EINTR (a signal came). FUTEX_WAKE: wakes at most val of the threads
 * waiting on uaddr and returns how many it woke. uaddr must be 4-byte
 * aligned; uaddr2 is unused. Another op fails with ENOSYS. */
int futex(volatile uint32_t *uaddr, int op, int val, const struct timespec *timeout,
		volatile uint32_t *uaddr2);

#ifdef __cplusplus
}
#endif

#endif /* _SYS_FUTEX_H */
