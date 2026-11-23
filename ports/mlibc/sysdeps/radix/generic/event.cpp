// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// kqueue(2) and kevent(2), sys/event.h's (ADR 0353): the event queue's
// one method, `wait`, is kevent itself, with its changes and its wait in
// one call. The kernel's QueueEvent is `struct kevent` as it stands.
//
// epoll(7) (ADR 0612): an epoll instance is an event queue too, and a
// registration its `poll` watch of a descriptor, which asks for Linux's
// events as they stand and reports those found in `fflags`: EPOLLET is the
// watch's EV_CLEAR, EPOLLONESHOT its EV_DISPATCH (a watch disabled once
// reported, until EPOLL_CTL_MOD enables it), and EPOLLEXCLUSIVE the
// kernel's own exclusive flag. The kernel's Linux table makes the same
// changes for a Linux program (kernel/lib/linux/src/epoll.rs).

#include <errno.h>
#include <mlibc/all-sysdeps.hpp>
#include <mlibc/threads.hpp>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <sys/epoll.h>
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

// The flags of `struct epoll_event` that are not events.
constexpr uint32_t EPOLL_INPUT_FLAGS = EPOLLEXCLUSIVE | EPOLLWAKEUP | EPOLLONESHOT | EPOLLET;

// What a registration with EPOLLEXCLUSIVE may ask for besides, as Linux's
// EPOLLEXCLUSIVE_OK_BITS.
constexpr uint32_t EPOLL_EXCLUSIVE_OK = EPOLLIN | EPOLLOUT | EPOLLERR | EPOLLHUP | EPOLLWAKEUP
		| EPOLLET | EPOLLEXCLUSIVE;

// The most events one epoll_wait reads from the kernel, on the stack: a
// call returns at most this many, as Linux's may return fewer than asked.
constexpr int EPOLL_AT_ONCE = 64;

// The most events epoll_wait takes, as Linux's EP_MAX_EVENTS.
constexpr int EPOLL_MAX_EVENTS = INT32_MAX / static_cast<int>(sizeof(struct epoll_event));

// The kernel's status for a call on a descriptor that is not an event
// queue as epoll's errno: EINVAL, where kevent says EBADF.
int epoll_errno(uint64_t status) {
	if(RADIX_STATUS_DOMAIN(status) == RADIX_KERNEL_ERROR_DOMAIN
			&& RADIX_STATUS_CODE(status) == RADIX_KERNEL_NO_SUCH_METHOD)
		return EINVAL;
	return radix::to_errno(status);
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

namespace mlibc {

int Sysdeps<EpollCreate>::operator()(int flags, int *fd) {
	if(flags & ~EPOLL_CLOEXEC)
		return EINVAL;
	radix_pipe_flags_t pipe_flags = (flags & EPOLL_CLOEXEC) ? RADIX_PIPE_FLAGS_CLOSE_ON_EXEC : 0;
	return radix::place(0, fd, [pipe_flags](uint32_t slot) {
		return radix_process_event_queue(radix::SLOT_PROCESS, slot, pipe_flags);
	});
}

int Sysdeps<EpollCtl>::operator()(int epfd, int mode, int fd, struct epoll_event *ev) {
	uint32_t flags;
	switch(mode) {
	case EPOLL_CTL_ADD:
		flags = RADIX_QUEUE_FLAGS_ADD | RADIX_QUEUE_FLAGS_CREATE | RADIX_QUEUE_FLAGS_ENABLE;
		break;
	case EPOLL_CTL_MOD:
		flags = RADIX_QUEUE_FLAGS_ENABLE;
		break;
	case EPOLL_CTL_DEL:
		flags = RADIX_QUEUE_FLAGS_DELETE;
		break;
	default:
		return EINVAL;
	}
	if(radix::slot_of(epfd) == radix::NO_SLOT || radix::slot_of(fd) == radix::NO_SLOT)
		return EBADF;
	struct radix_queue_event change;
	memset(&change, 0, sizeof(change));
	change.ident = static_cast<uint64_t>(fd);
	change.filter = RADIX_QUEUE_FILTER_POLL;
	if(mode != EPOLL_CTL_DEL) {
		if(!ev)
			return EFAULT;
		uint32_t events = ev->events;
		if((events & EPOLLEXCLUSIVE)
				&& (mode == EPOLL_CTL_MOD || (events & ~EPOLL_EXCLUSIVE_OK)))
			return EINVAL;
		if(events & EPOLLET)
			flags |= RADIX_QUEUE_FLAGS_CLEAR;
		if(events & EPOLLONESHOT)
			flags |= RADIX_QUEUE_FLAGS_DISPATCH;
		if(events & EPOLLEXCLUSIVE)
			flags |= RADIX_QUEUE_FLAGS_EXCLUSIVE;
		// EPOLLWAKEUP is dropped, as Linux drops it for a program without
		// CAP_BLOCK_SUSPEND.
		change.fflags = events & ~EPOLL_INPUT_FLAGS;
		change.udata = ev->data.u64;
	}
	change.flags = static_cast<uint16_t>(flags);
	// With no room for events, a change that fails fails the call.
	struct radix_event_queue_wait_results r;
	uint64_t status = radix_event_queue_wait(radix::slot_of(epfd),
			reinterpret_cast<uint64_t>(&change), 1, 0, 0, radix::FD_BASE, 0, 0, 0, &r);
	return status ? epoll_errno(status) : 0;
}

int Sysdeps<EpollPwait>::operator()(int epfd, struct epoll_event *ev, int n, int timeout,
		const sigset_t *sigmask, int *raised) {
	if(n <= 0 || n > EPOLL_MAX_EVENTS)
		return EINVAL;
	uint32_t flags = 0;
	uint64_t ns = 0;
	if(timeout < 0)
		flags |= RADIX_QUEUE_WAIT_FLAGS_FOREVER;
	else
		ns = static_cast<uint64_t>(timeout) * 1000000;
	uint64_t mask = 0;
	if(sigmask) {
		flags |= RADIX_QUEUE_WAIT_FLAGS_MASK;
		memcpy(&mask, sigmask, sizeof(mask));
	}
	struct radix_queue_event found[EPOLL_AT_ONCE];
	auto capacity = static_cast<uint32_t>(n < EPOLL_AT_ONCE ? n : EPOLL_AT_ONCE);
	struct radix_event_queue_wait_results r;
	uint64_t status = radix_event_queue_wait_with(radix::cancellable_masked, radix::slot_of(epfd),
			0, 0, reinterpret_cast<uint64_t>(found), capacity, radix::FD_BASE, ns, mask, flags,
			&r);
	if(status == radix::CANCELED)
		mlibc::thread_testcancel();
	if(status)
		return epoll_errno(status);
	for(uint32_t i = 0; i < r.found; i++) {
		ev[i].events = found[i].fflags;
		ev[i].data.u64 = found[i].udata;
	}
	*raised = static_cast<int>(r.found);
	return 0;
}

} // namespace mlibc
