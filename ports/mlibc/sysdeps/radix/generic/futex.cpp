// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// futex(2), sys/futex.h's (ADR 0350): the address space's futex_wait and
// futex_wake methods, which the C library's own locks use too, for
// programs that keep theirs themselves.

#include <errno.h>
#include <stdint.h>
#include <sys/futex.h>
#include <time.h>

#include "radix.hpp"

namespace {

constexpr uint64_t NS_PER_SEC = 1000000000;

// The kernel's timeout for `timeout`, in nanoseconds, or EINVAL if it is
// not a time. The kernel takes 0 for no limit, so none is 0 and a zero
// time the shortest wait.
int timeout_ns(const struct timespec *timeout, uint64_t *ns) {
	if(!timeout) {
		*ns = 0;
		return 0;
	}
	if(timeout->tv_sec < 0 || timeout->tv_nsec < 0
			|| timeout->tv_nsec >= static_cast<long>(NS_PER_SEC))
		return EINVAL;
	auto secs = static_cast<uint64_t>(timeout->tv_sec);
	auto nanos = static_cast<uint64_t>(timeout->tv_nsec);
	if(secs > (UINT64_MAX - nanos) / NS_PER_SEC)
		*ns = UINT64_MAX;
	else
		*ns = secs * NS_PER_SEC + nanos;
	if(!*ns)
		*ns = 1;
	return 0;
}

} // namespace

int futex(volatile uint32_t *uaddr, int op, int val, const struct timespec *timeout,
		volatile uint32_t *uaddr2) {
	(void)uaddr2;
	auto address = reinterpret_cast<uint64_t>(uaddr);
	int e;
	switch(op & ~FUTEX_PRIVATE_FLAG) {
	case FUTEX_WAIT: {
		uint64_t ns;
		e = timeout_ns(timeout, &ns);
		if(!e)
			e = radix::to_errno(radix_address_space_futex_wait(radix::SLOT_SPACE, address,
					static_cast<uint32_t>(val), ns));
		if(!e)
			return 0;
		break;
	}
	case FUTEX_WAKE: {
		if(val < 0) {
			e = EINVAL;
			break;
		}
		if(!val)
			return 0;
		struct radix_address_space_futex_wake_results r;
		e = radix::to_errno(radix_address_space_futex_wake(radix::SLOT_SPACE, address,
				static_cast<uint32_t>(val), &r));
		if(!e)
			return static_cast<int>(r.woken);
		break;
	}
	default:
		e = ENOSYS;
		break;
	}
	errno = e;
	return -1;
}
