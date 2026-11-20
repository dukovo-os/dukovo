// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// kqueue(2) and kevent(2), sys/event.h's (ADR 0353): the event queue's
// one method, `wait`, is kevent itself, with its changes and its wait in
// one call. The kernel's QueueEvent is `struct kevent` as it stands.

#include <errno.h>
#include <mlibc/threads.hpp>
#include <stddef.h>
#include <stdint.h>
#include <sys/event.h>
#include <time.h>

#include "radix.hpp"

namespace {

static_assert(sizeof(struct kevent) == sizeof(struct radix_queue_event), "kevent is the kernel's");
static_assert(offsetof(struct kevent, ident) == offsetof(struct radix_queue_event, ident),
		"kevent's ident is the kernel's");
static_assert(offsetof(struct kevent, filter) == offsetof(struct radix_queue_event, filter),
		"kevent's filter is the kernel's");
static_assert(offsetof(struct kevent, flags) == offsetof(struct radix_queue_event, flags),
		"kevent's flags are the kernel's");
static_assert(offsetof(struct kevent, fflags) == offsetof(struct radix_queue_event, fflags),
		"kevent's fflags are the kernel's");
static_assert(offsetof(struct kevent, data) == offsetof(struct radix_queue_event, data),
		"kevent's data is the kernel's");
static_assert(offsetof(struct kevent, udata) == offsetof(struct radix_queue_event, udata),
		"kevent's udata is the kernel's");

constexpr uint64_t NS_PER_SEC = 1000000000;

// How the kernel is to wait for `timeout`: its nanoseconds in `ns`, or
// FOREVER in `flags` for none, and for one too long for 64 bits of
// nanoseconds. A zero time is a wait of 0 nanoseconds, which does not
// wait. Returns EINVAL if `timeout` is not a time.
int wait_time(const struct timespec *timeout, uint64_t *ns, radix_queue_wait_flags_t *flags) {
	*ns = 0;
	*flags = 0;
	if(!timeout) {
		*flags = RADIX_QUEUE_WAIT_FLAGS_FOREVER;
		return 0;
	}
	if(timeout->tv_sec < 0 || timeout->tv_nsec < 0
			|| timeout->tv_nsec >= static_cast<long>(NS_PER_SEC))
		return EINVAL;
	auto secs = static_cast<uint64_t>(timeout->tv_sec);
	auto nanos = static_cast<uint64_t>(timeout->tv_nsec);
	if(secs > (UINT64_MAX - nanos) / NS_PER_SEC)
		*flags = RADIX_QUEUE_WAIT_FLAGS_FOREVER;
	else
		*ns = secs * NS_PER_SEC + nanos;
	return 0;
}

} // namespace

int kqueue(void) {
	int fd;
	int e = radix::place(0, &fd, [](uint32_t slot) {
		return radix_process_create(radix::SLOT_PROCESS, RADIX_OBJECT_KIND_EVENT_QUEUE, 0, slot);
	});
	if(e) {
		errno = e;
		return -1;
	}
	return fd;
}

int kevent(int kq, const struct kevent *changelist, int nchanges, struct kevent *eventlist,
		int nevents, const struct timespec *timeout) {
	if(nchanges < 0 || nchanges > static_cast<int>(RADIX_EVENT_QUEUE_CHANGES_MAX) || nevents < 0) {
		errno = EINVAL;
		return -1;
	}
	uint64_t ns;
	radix_queue_wait_flags_t flags;
	if(int e = wait_time(timeout, &ns, &flags); e) {
		errno = e;
		return -1;
	}
	auto changes = reinterpret_cast<uint64_t>(changelist);
	auto events = reinterpret_cast<uint64_t>(eventlist);
	auto count = static_cast<uint32_t>(nchanges);
	auto capacity = static_cast<uint32_t>(nevents);
	struct radix_event_queue_wait_results r;
	uint64_t status;
	// A call with no room for events does not wait, and is not a
	// cancellation point, as on FreeBSD. kevent has no signal mask to give
	// the kernel.
	if(capacity) {
		status = radix_event_queue_wait_with(radix::cancellable_masked, radix::slot_of(kq),
				changes, count, events, capacity, radix::FD_BASE, ns, 0, flags, &r);
	} else {
		status = radix_event_queue_wait(radix::slot_of(kq), changes, count, events, capacity,
				radix::FD_BASE, ns, 0, flags, &r);
	}
	if(status == radix::CANCELED)
		mlibc::thread_testcancel();
	// Another kind of descriptor has no `wait` method; the BSDs' kevent
	// says EBADF for a descriptor that is not a queue.
	if(RADIX_STATUS_DOMAIN(status) == RADIX_KERNEL_ERROR_DOMAIN
			&& RADIX_STATUS_CODE(status) == RADIX_KERNEL_NO_SUCH_METHOD) {
		errno = EBADF;
		return -1;
	}
	if(status) {
		errno = radix::to_errno(status);
		return -1;
	}
	// The kernel puts the status of a change that failed in `data`; the
	// program is given its errno, as on the BSDs.
	for(uint32_t i = 0; i < r.found; i++) {
		if(eventlist[i].flags & EV_ERROR)
			eventlist[i].data = radix::to_errno(static_cast<uint64_t>(eventlist[i].data));
	}
	return static_cast<int>(r.found);
}
