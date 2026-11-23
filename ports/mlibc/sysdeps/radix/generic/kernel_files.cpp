// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The kernel's own files on Radix (ADR 0613): eventfd, timerfd, signalfd
// and pidfd, of the Linux option's generic code, which calls the sysdeps
// here. Each is the process's `event_counter`, `timer_file`, `signal_file`
// or `process_file`, which puts a file in the lowest free descriptor; reads
// and writes are the descriptor's own (`Read` and `Write`), and the
// kernel's records are Linux's (`struct signalfd_siginfo`, an 8-byte
// counter, `struct itimerspec`), so the code here only converts what a
// call passes. The errors are Linux's, and in its order.

#include <abi-bits/fcntl.h>
#include <errno.h>
#include <fcntl.h>
#include <mlibc/all-sysdeps.hpp>
#include <signal.h>
#include <stdint.h>
#include <string.h>
#include <time.h>

#include "radix.hpp"

namespace {

constexpr uint64_t NS_PER_SEC = 1000000000;

// EFD_SEMAPHORE, which the eventfd header of the Linux option defines too:
// kept here so that this file needs none of its headers.
constexpr int COUNTER_SEMAPHORE = 1;
constexpr int TIMER_ABSOLUTE = 1;
constexpr int TIMER_CANCEL_ON_SET = 2;
constexpr int FILE_FLAGS = O_NONBLOCK | O_CLOEXEC;
// The alarm clocks, which need a capability no process has.
constexpr int CLOCK_REALTIME_ALARM_NUMBER = 8;
constexpr int CLOCK_BOOTTIME_ALARM_NUMBER = 9;
constexpr unsigned int PIDFD_NONBLOCK_FLAG = O_NONBLOCK;

// The kernel's PipeFlags for O_NONBLOCK and O_CLOEXEC in `flags`.
uint32_t description_flags(int flags) {
	uint32_t kernel = 0;
	if(flags & O_NONBLOCK)
		kernel |= RADIX_PIPE_FLAGS_NON_BLOCKING;
	if(flags & O_CLOEXEC)
		kernel |= RADIX_PIPE_FLAGS_CLOSE_ON_EXEC;
	return kernel;
}

// Whether `status` is the kernel error `code`.
bool is(uint64_t status, uint16_t code) {
	return RADIX_STATUS_DOMAIN(status) == RADIX_KERNEL_ERROR_DOMAIN
			&& RADIX_STATUS_CODE(status) == code;
}

// The errno of a call that a file of another kind answered with
// no_such_method, wrong_kind or invalid_argument, which is what each
// method says of a file that is not the one it is for: `other`.
int kind_errno(uint64_t status, int other) {
	if(is(status, RADIX_KERNEL_NO_SUCH_METHOD) || is(status, RADIX_KERNEL_WRONG_KIND)
			|| is(status, RADIX_KERNEL_INVALID_ARGUMENT))
		return other;
	return radix::to_errno(status);
}

// `secs` seconds and `ns` nanoseconds as nanoseconds, at most UINT64_MAX:
// some 584 years, which the kernel takes as never, as Linux keeps a time
// past its 292.
uint64_t saturating_ns(uint64_t secs, uint64_t ns) {
	if(secs > (UINT64_MAX - ns) / NS_PER_SEC)
		return UINT64_MAX;
	return secs * NS_PER_SEC + ns;
}

// Nanoseconds in `ts`, or EINVAL if it is not a time.
int timespec_ns(const struct timespec &ts, uint64_t *ns) {
	if(ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= static_cast<long>(NS_PER_SEC))
		return EINVAL;
	*ns = saturating_ns(static_cast<uint64_t>(ts.tv_sec), static_cast<uint64_t>(ts.tv_nsec));
	return 0;
}

struct timespec timespec_of(uint64_t ns) {
	struct timespec ts;
	ts.tv_sec = static_cast<time_t>(ns / NS_PER_SEC);
	ts.tv_nsec = static_cast<long>(ns % NS_PER_SEC);
	return ts;
}

// An `itimerspec` of the kernel's two times.
struct itimerspec itimerspec_of(uint64_t value_ns, uint64_t interval_ns) {
	struct itimerspec its;
	its.it_interval = timespec_of(interval_ns);
	its.it_value = timespec_of(value_ns);
	return its;
}

// A set of signals as the kernel takes one: bit n - 1 for signal n.
uint64_t mask_of(const sigset_t *set) {
	uint64_t mask;
	memcpy(&mask, set, sizeof(mask));
	return mask;
}

// The number of the process the process file in descriptor `fd` is of,
// and whether the file does not wait. EBADF for a descriptor that is not
// open or not a process file.
int process_of(int fd, pid_t *pid, bool *non_blocking) {
	uint32_t slot = radix::slot_of(fd);
	if(slot == radix::NO_SLOT)
		return EBADF;
	struct radix_file_process_of_results r;
	if(uint64_t status = radix_file_process_of(slot, &r); status)
		return kind_errno(status, EBADF);
	*pid = static_cast<pid_t>(r.pid);
	if(non_blocking) {
		struct radix_file_status_flags_results flags;
		if(uint64_t status = radix_file_status_flags(slot, false, 0, &flags); status)
			return radix::to_errno(status);
		*non_blocking = flags.flags & RADIX_DESCRIPTION_FLAGS_NON_BLOCKING;
	}
	return 0;
}

} // namespace

namespace mlibc {

int Sysdeps<EventfdCreate>::operator()(unsigned int initval, int flags, int *fd) {
	if(flags & ~(COUNTER_SEMAPHORE | FILE_FLAGS))
		return EINVAL;
	uint32_t kernel = 0;
	if(flags & O_NONBLOCK)
		kernel |= RADIX_COUNTER_FLAGS_NON_BLOCKING;
	if(flags & O_CLOEXEC)
		kernel |= RADIX_COUNTER_FLAGS_CLOSE_ON_EXEC;
	if(flags & COUNTER_SEMAPHORE)
		kernel |= RADIX_COUNTER_FLAGS_SEMAPHORE;
	return radix::place(0, fd, [&](uint32_t slot) {
		return radix_process_event_counter(radix::SLOT_PROCESS, slot, initval, kernel);
	});
}

int Sysdeps<TimerfdCreate>::operator()(int clockid, int flags, int *fd) {
	if(flags & ~FILE_FLAGS)
		return EINVAL;
	if(clockid == CLOCK_REALTIME_ALARM_NUMBER || clockid == CLOCK_BOOTTIME_ALARM_NUMBER)
		return EPERM;
	// Only the three clocks Linux's timer files take besides the alarm
	// ones; any other is EINVAL, as on Linux.
	if(clockid != CLOCK_REALTIME && clockid != CLOCK_MONOTONIC && clockid != CLOCK_BOOTTIME)
		return EINVAL;
	enum radix_clock_kind kind;
	if(int e = radix::clock_kind(clockid, &kind); e)
		return e;
	uint32_t kernel = description_flags(flags);
	return radix::place(0, fd, [&](uint32_t slot) {
		return radix_process_timer_file(radix::SLOT_PROCESS, slot, kind, kernel);
	});
}

int Sysdeps<TimerfdSettime>::operator()(int fd, int flags, const struct itimerspec *value,
		struct itimerspec *oldvalue) {
	// Linux copies the new setting in first, then checks the flags and it.
	struct itimerspec its;
	memcpy(&its, value, sizeof(its));
	if(flags & ~(TIMER_ABSOLUTE | TIMER_CANCEL_ON_SET))
		return EINVAL;
	uint64_t value_ns;
	uint64_t interval_ns;
	if(int e = timespec_ns(its.it_value, &value_ns); e)
		return e;
	if(int e = timespec_ns(its.it_interval, &interval_ns); e)
		return e;
	uint32_t slot = radix::slot_of(fd);
	if(slot == radix::NO_SLOT)
		return EBADF;
	uint32_t kernel = 0;
	if(flags & TIMER_ABSOLUTE)
		kernel |= RADIX_TIMER_SET_FLAGS_ABSOLUTE;
	if(flags & TIMER_CANCEL_ON_SET)
		kernel |= RADIX_TIMER_SET_FLAGS_CANCEL_ON_SET;
	struct radix_file_timer_set_results r;
	if(uint64_t status = radix_file_timer_set(slot, kernel, value_ns, interval_ns, &r); status)
		return kind_errno(status, EINVAL);
	if(oldvalue)
		*oldvalue = itimerspec_of(r.value_ns, r.interval_ns);
	return 0;
}

int Sysdeps<TimerfdGettime>::operator()(int fd, struct itimerspec *its) {
	uint32_t slot = radix::slot_of(fd);
	if(slot == radix::NO_SLOT)
		return EBADF;
	struct radix_file_timer_get_results r;
	if(uint64_t status = radix_file_timer_get(slot, &r); status)
		return kind_errno(status, EINVAL);
	*its = itimerspec_of(r.value_ns, r.interval_ns);
	return 0;
}

// `*fd` is the descriptor to change the mask of, or -1 for a new one,
// as mlibc's signalfd leaves it.
int Sysdeps<SignalfdCreate>::operator()(const sigset_t *mask, int flags, int *fd) {
	if(flags & ~FILE_FLAGS)
		return EINVAL;
	uint64_t wanted = mask_of(mask);
	if(*fd != -1) {
		uint32_t slot = radix::slot_of(*fd);
		if(slot == radix::NO_SLOT)
			return EBADF;
		return kind_errno(radix_file_set_signal_mask(slot, wanted), EINVAL);
	}
	uint32_t kernel = description_flags(flags);
	return radix::place(0, fd, [&](uint32_t slot) {
		return radix_process_signal_file(radix::SLOT_PROCESS, slot, wanted, kernel);
	});
}

int Sysdeps<PidfdOpen>::operator()(pid_t pid, unsigned int flags, int *fd) {
	if((flags & ~PIDFD_NONBLOCK_FLAG) || pid <= 0)
		return EINVAL;
	// Closed by `exec`, as Linux's are.
	uint32_t kernel = RADIX_PIPE_FLAGS_CLOSE_ON_EXEC;
	if(flags & PIDFD_NONBLOCK_FLAG)
		kernel |= RADIX_PIPE_FLAGS_NON_BLOCKING;
	return radix::place(0, fd, [&](uint32_t slot) {
		return radix_process_process_file(radix::SLOT_PROCESS, static_cast<uint64_t>(pid), slot,
				kernel);
	});
}

int Sysdeps<PidfdGetpid>::operator()(int fd, pid_t *outpid) {
	return process_of(fd, outpid, nullptr);
}

int Sysdeps<PidfdSendSignal>::operator()(int pidfd, int sig, siginfo_t *info,
		unsigned int flags) {
	if(flags)
		return EINVAL;
	pid_t pid;
	if(int e = process_of(pidfd, &pid, nullptr); e)
		return e;
	// The record is checked before the signal, as Linux checks them.
	if(info) {
		siginfo_t record;
		memcpy(&record, info, sizeof(record));
		if(record.si_signo != sig)
			return EINVAL;
		// Linux lets a program fake only a sigqueue signal to others.
		if(pid != radix::pid() && (record.si_code >= 0 || record.si_code == SI_TKILL))
			return EPERM;
		if(sig < 0 || sig > 64)
			return EINVAL;
		if(sig) {
			uint64_t value = 0;
			memcpy(&value, &record.si_value, sizeof(record.si_value));
			return radix::to_errno(radix_process_signal_queue(radix::SLOT_PROCESS,
					static_cast<uint64_t>(pid), static_cast<uint32_t>(sig), value));
		}
	}
	if(sig < 0 || sig > 64)
		return EINVAL;
	return radix::to_errno(radix_process_kill(radix::SLOT_PROCESS, pid,
			static_cast<uint32_t>(sig)));
}

} // namespace mlibc
