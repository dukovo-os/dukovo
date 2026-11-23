// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The kernel's own files (ADR 0613): eventfd's counters, timerfd's timers,
// signalfd's signals and pidfd's processes. Each is checked as Linux
// answers it: what a read gives and when it waits, what a poll says, the
// records, and the errors in the order Linux gives them, so that the
// program passes on a Linux machine as it is.

#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <sys/eventfd.h>
#include <sys/signalfd.h>
#include <sys/syscall.h>
#include <sys/timerfd.h>

#include "check.h"

#ifndef P_PIDFD
#define P_PIDFD 3
#endif

// pidfd_open and pidfd_send_signal, which a C library may lack: Dukovo's
// has them, and Linux's system calls are the same.
static int open_pidfd(pid_t pid, unsigned int flags) {
	return (int)syscall(SYS_pidfd_open, pid, flags);
}

static int signal_pidfd(int pidfd, int sig, siginfo_t *info, unsigned int flags) {
	return (int)syscall(SYS_pidfd_send_signal, pidfd, sig, info, flags);
}

// Whether `fd` says it is ready for `events` within `ms` milliseconds:
// the events it says, or 0.
static int ready(int fd, short events, int ms) {
	struct pollfd p = {fd, events, 0};
	int n = poll(&p, 1, ms);
	CHECK(n >= 0);
	return n > 0 ? p.revents : 0;
}

// Reads an eventfd's or timerfd's count from `fd`: -1 and errno if the
// read fails.
static int64_t count_of(int fd) {
	uint64_t count = 0;
	ssize_t n = read(fd, &count, sizeof(count));
	if(n != (ssize_t)sizeof(count))
		return -1;
	return (int64_t)count;
}

static struct timespec time_of(long sec, long nsec) {
	struct timespec ts = {sec, nsec};
	return ts;
}

static struct itimerspec timer_of(long value_ms, long interval_ms) {
	struct itimerspec its;
	its.it_value = time_of(value_ms / 1000, (value_ms % 1000) * 1000000L);
	its.it_interval = time_of(interval_ms / 1000, (interval_ms % 1000) * 1000000L);
	return its;
}

static long long nanoseconds(const struct timespec *ts) {
	return (long long)ts->tv_sec * 1000000000LL + ts->tv_nsec;
}

static void counters(void) {
	// The count adds up, and one read takes it all.
	int fd = eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC);
	CHECK(fd >= 0);
	CHECK(fcntl(fd, F_GETFD) & FD_CLOEXEC);
	CHECK(fcntl(fd, F_GETFL) & O_NONBLOCK);
	CHECK_ERROR(count_of(fd), EAGAIN);
	CHECK(ready(fd, POLLIN, 0) == 0);
	CHECK(ready(fd, POLLOUT, 0) == POLLOUT);
	uint64_t value = 3;
	CHECK(write(fd, &value, sizeof(value)) == sizeof(value));
	value = 4;
	CHECK(eventfd_write(fd, value) == 0);
	CHECK(ready(fd, POLLIN, 0) & POLLIN);
	CHECK(count_of(fd) == 7);
	CHECK_ERROR(count_of(fd), EAGAIN);
	// A counter that begins above 0 is readable at once.
	int started = eventfd(5, EFD_NONBLOCK);
	CHECK(started >= 0);
	CHECK(ready(started, POLLIN, 0) & POLLIN);
	eventfd_t got = 0;
	CHECK(eventfd_read(started, &got) == 0 && got == 5);
	close(started);

	// The semaphore's reads take 1 each.
	int semaphore = eventfd(2, EFD_SEMAPHORE | EFD_NONBLOCK);
	CHECK(semaphore >= 0);
	CHECK(count_of(semaphore) == 1);
	CHECK(count_of(semaphore) == 1);
	CHECK_ERROR(count_of(semaphore), EAGAIN);
	value = 10;
	CHECK(write(semaphore, &value, sizeof(value)) == sizeof(value));
	CHECK(count_of(semaphore) == 1);
	CHECK(ready(semaphore, POLLIN, 0) & POLLIN);
	close(semaphore);

	// Sizes and values: a read or write of other than 8 bytes, and the
	// largest count, are invalid.
	char small[4] = {0};
	CHECK_ERROR(read(fd, small, sizeof(small)), EINVAL);
	CHECK_ERROR(write(fd, small, sizeof(small)), EINVAL);
	value = UINT64_MAX;
	CHECK_ERROR(write(fd, &value, sizeof(value)), EINVAL);
	CHECK_ERROR(eventfd(0, 0x40000000), EINVAL);

	// A full counter does not take more: a write that would pass the
	// largest waits, which a timeout cuts short, and a non-blocking one
	// fails.
	value = UINT64_MAX - 1;
	CHECK(write(fd, &value, sizeof(value)) == sizeof(value));
	CHECK(ready(fd, POLLOUT, 0) == 0);
	value = 1;
	CHECK_ERROR(write(fd, &value, sizeof(value)), EAGAIN);
	CHECK(ready(fd, POLLOUT, 50) == 0);
	CHECK(count_of(fd) == (int64_t)(UINT64_MAX - 1));
	CHECK(ready(fd, POLLOUT, 0) == POLLOUT);
	close(fd);
}

static void counter_waits(void) {
	int fd = eventfd(0, 0);
	CHECK(fd >= 0);
	int again = eventfd(0, EFD_SEMAPHORE);
	CHECK(again >= 0);
	// A read waits for a write of another process, and wakes the poll too.
	pid_t child = check_fork();
	if(child == 0) {
		check_sleep_ms(40);
		CHECK(eventfd_write(fd, 5) == 0);
		check_sleep_ms(40);
		CHECK(eventfd_write(again, 2) == 0);
		check_exit_child();
	}
	long long start = check_now_ns();
	CHECK(count_of(fd) == 5);
	long long waited = check_now_ns() - start;
	CHECK(waited >= 20000000LL && waited < 5000000000LL);
	CHECK(ready(again, POLLIN, 5000) & POLLIN);
	CHECK(count_of(again) == 1);
	CHECK(count_of(again) == 1);
	CHECK_CHILD(child);
	// A description shared by dup is one counter.
	int copy = dup(fd);
	CHECK(copy >= 0);
	CHECK(eventfd_write(copy, 9) == 0);
	CHECK(count_of(fd) == 9);
	close(copy);
	close(fd);
	close(again);
}

static void timers(void) {
	CHECK_ERROR(timerfd_create(CLOCK_MONOTONIC, 0x40000000), EINVAL);
	CHECK_ERROR(timerfd_create(CLOCK_PROCESS_CPUTIME_ID, 0), EINVAL);
	CHECK_ERROR(timerfd_create(-1, 0), EINVAL);
	int fd = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK | TFD_CLOEXEC);
	CHECK(fd >= 0);
	CHECK(fcntl(fd, F_GETFD) & FD_CLOEXEC);
	// Unarmed, it never expires.
	CHECK_ERROR(count_of(fd), EAGAIN);
	CHECK(ready(fd, POLLIN, 0) == 0);
	struct itimerspec its = timer_of(0, 0);
	CHECK(timerfd_gettime(fd, &its) == 0);
	CHECK(its.it_value.tv_sec == 0 && its.it_value.tv_nsec == 0);
	CHECK(its.it_interval.tv_sec == 0 && its.it_interval.tv_nsec == 0);

	// A relative time: a read waits for it, and takes 1.
	struct itimerspec set = timer_of(30, 0);
	struct itimerspec old = timer_of(99, 99);
	CHECK(timerfd_settime(fd, 0, &set, &old) == 0);
	CHECK(old.it_value.tv_sec == 0 && old.it_value.tv_nsec == 0);
	CHECK(ready(fd, POLLIN, 0) == 0);
	long long start = check_now_ns();
	CHECK(ready(fd, POLLIN, 5000) & POLLIN);
	long long waited = check_now_ns() - start;
	CHECK(waited >= 15000000LL && waited < 5000000000LL);
	CHECK(count_of(fd) == 1);
	CHECK_ERROR(count_of(fd), EAGAIN);
	// One shot: it does not expire again, and reads as disarmed.
	CHECK(timerfd_gettime(fd, &its) == 0);
	CHECK(its.it_value.tv_sec == 0 && its.it_value.tv_nsec == 0);
	check_sleep_ms(30);
	CHECK_ERROR(count_of(fd), EAGAIN);

	// An interval counts every expiry that nobody read.
	set = timer_of(10, 10);
	CHECK(timerfd_settime(fd, 0, &set, NULL) == 0);
	check_sleep_ms(105);
	int64_t ticks = count_of(fd);
	CHECK(ticks >= 9 && ticks <= 40);
	CHECK_ERROR(count_of(fd), EAGAIN);
	CHECK(timerfd_gettime(fd, &its) == 0);
	CHECK(its.it_interval.tv_sec == 0 && its.it_interval.tv_nsec == 10000000L);
	CHECK(nanoseconds(&its.it_value) > 0 && nanoseconds(&its.it_value) <= 10000000LL);
	// Setting it again counts afresh, and gives back the old setting.
	set = timer_of(1000, 500);
	CHECK(timerfd_settime(fd, 0, &set, &old) == 0);
	CHECK(old.it_interval.tv_nsec == 10000000L && old.it_interval.tv_sec == 0);
	CHECK(nanoseconds(&old.it_value) > 0 && nanoseconds(&old.it_value) <= 10000000LL);
	CHECK(timerfd_gettime(fd, &its) == 0);
	CHECK(nanoseconds(&its.it_value) > 500000000LL && nanoseconds(&its.it_value) <= 1000000000LL);
	CHECK(its.it_interval.tv_sec == 0 && its.it_interval.tv_nsec == 500000000L);
	CHECK(ready(fd, POLLIN, 0) == 0);
	// Zero disarms it, and gives back what was left.
	set = timer_of(0, 0);
	CHECK(timerfd_settime(fd, 0, &set, &old) == 0);
	CHECK(nanoseconds(&old.it_value) > 0);
	CHECK(timerfd_gettime(fd, &its) == 0);
	CHECK(nanoseconds(&its.it_value) == 0 && nanoseconds(&its.it_interval) == 0);

	// An absolute time on the timer's clock, one past too.
	struct timespec now;
	CHECK(clock_gettime(CLOCK_MONOTONIC, &now) == 0);
	set = timer_of(0, 0);
	set.it_value = time_of(now.tv_sec, now.tv_nsec + 40000000L);
	if(set.it_value.tv_nsec >= 1000000000L) {
		set.it_value.tv_sec++;
		set.it_value.tv_nsec -= 1000000000L;
	}
	CHECK(timerfd_settime(fd, TFD_TIMER_ABSTIME, &set, NULL) == 0);
	CHECK(ready(fd, POLLIN, 0) == 0);
	CHECK(ready(fd, POLLIN, 5000) & POLLIN);
	CHECK(count_of(fd) == 1);
	set.it_value = time_of(now.tv_sec - 1, 0);
	CHECK(timerfd_settime(fd, TFD_TIMER_ABSTIME, &set, NULL) == 0);
	CHECK(ready(fd, POLLIN, 0) & POLLIN);
	CHECK(count_of(fd) == 1);
	close(fd);

	// A blocking read waits for the expiry, and the realtime clock has
	// timers too.
	int blocking = timerfd_create(CLOCK_REALTIME, 0);
	CHECK(blocking >= 0);
	set = timer_of(30, 0);
	CHECK(timerfd_settime(blocking, 0, &set, NULL) == 0);
	start = check_now_ns();
	CHECK(count_of(blocking) == 1);
	waited = check_now_ns() - start;
	CHECK(waited >= 15000000LL && waited < 5000000000LL);
	close(blocking);
	int boot = timerfd_create(CLOCK_BOOTTIME, TFD_NONBLOCK);
	CHECK(boot >= 0);
	close(boot);
}

static void timer_errors(void) {
	int fd = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK);
	CHECK(fd >= 0);
	struct itimerspec good = timer_of(1000, 0);
	// A flag it does not have, and times that are not times.
	CHECK_ERROR(timerfd_settime(fd, 0x40, &good, NULL), EINVAL);
	struct itimerspec bad = good;
	bad.it_value.tv_nsec = 1000000000L;
	CHECK_ERROR(timerfd_settime(fd, 0, &bad, NULL), EINVAL);
	bad = good;
	bad.it_interval.tv_nsec = -1;
	CHECK_ERROR(timerfd_settime(fd, 0, &bad, NULL), EINVAL);
	bad = good;
	bad.it_value.tv_sec = -1;
	CHECK_ERROR(timerfd_settime(fd, 0, &bad, NULL), EINVAL);
	// Nothing of that took.
	struct itimerspec its;
	CHECK(timerfd_gettime(fd, &its) == 0);
	CHECK(nanoseconds(&its.it_value) == 0);
	// A read of fewer than 8 bytes.
	char small[4];
	CHECK_ERROR(read(fd, small, sizeof(small)), EINVAL);
	// A timer file takes no writes.
	uint64_t value = 1;
	CHECK_ERROR(write(fd, &value, sizeof(value)), EINVAL);
	// Descriptors that are not timer files, and none.
	int counter = eventfd(0, 0);
	CHECK(counter >= 0);
	CHECK_ERROR(timerfd_settime(counter, 0, &good, NULL), EINVAL);
	CHECK_ERROR(timerfd_gettime(counter, &its), EINVAL);
	CHECK_ERROR(timerfd_settime(-1, 0, &good, NULL), EBADF);
	CHECK_ERROR(timerfd_gettime(-1, &its), EBADF);
	CHECK_ERROR(timerfd_settime(9999, 0, &good, NULL), EBADF);
	close(counter);
	close(fd);
}

static void signals(void) {
	sigset_t set;
	sigemptyset(&set);
	sigaddset(&set, SIGUSR1);
	CHECK(sigprocmask(SIG_BLOCK, &set, NULL) == 0);
	int fd = signalfd(-1, &set, SFD_NONBLOCK | SFD_CLOEXEC);
	CHECK(fd >= 0);
	CHECK(fcntl(fd, F_GETFD) & FD_CLOEXEC);
	struct signalfd_siginfo info;
	CHECK_ERROR(read(fd, &info, sizeof(info)), EAGAIN);
	CHECK(ready(fd, POLLIN, 0) == 0);

	// A raised signal is readable, and the read takes it.
	CHECK(kill(getpid(), SIGUSR1) == 0);
	CHECK(ready(fd, POLLIN, 0) & POLLIN);
	memset(&info, 0xff, sizeof(info));
	CHECK(read(fd, &info, sizeof(info)) == sizeof(info));
	CHECK(info.ssi_signo == SIGUSR1);
	CHECK(info.ssi_code == SI_USER);
	CHECK(info.ssi_pid == (uint32_t)getpid());
	CHECK(info.ssi_errno == 0 && info.ssi_status == 0);
	CHECK(ready(fd, POLLIN, 0) == 0);
	sigset_t pending;
	CHECK(sigpending(&pending) == 0 && !sigismember(&pending, SIGUSR1));
	CHECK_ERROR(read(fd, &info, sizeof(info)), EAGAIN);

	// A queued signal brings its value.
	union sigval value;
	value.sival_int = 1234;
	CHECK(sigqueue(getpid(), SIGUSR1, value) == 0);
	memset(&info, 0xff, sizeof(info));
	CHECK(read(fd, &info, sizeof(info)) == sizeof(info));
	CHECK(info.ssi_signo == SIGUSR1 && info.ssi_code == SI_QUEUE);
	CHECK(info.ssi_int == 1234);
	CHECK(info.ssi_pid == (uint32_t)getpid());

	// A read of fewer than 128 bytes.
	CHECK(kill(getpid(), SIGUSR1) == 0);
	char small[64];
	CHECK_ERROR(read(fd, small, sizeof(small)), EINVAL);
	CHECK(read(fd, &info, sizeof(info)) == sizeof(info));
	close(fd);
	sigprocmask(SIG_UNBLOCK, &set, NULL);
}

static void signal_masks(void) {
	sigset_t both;
	sigemptyset(&both);
	sigaddset(&both, SIGUSR1);
	sigaddset(&both, SIGUSR2);
	CHECK(sigprocmask(SIG_BLOCK, &both, NULL) == 0);
	sigset_t only_first;
	sigemptyset(&only_first);
	sigaddset(&only_first, SIGUSR1);
	int fd = signalfd(-1, &only_first, SFD_NONBLOCK);
	CHECK(fd >= 0);

	// A signal outside the mask stays pending and unread.
	struct signalfd_siginfo info[4];
	CHECK(kill(getpid(), SIGUSR2) == 0);
	CHECK(ready(fd, POLLIN, 0) == 0);
	CHECK_ERROR(read(fd, info, sizeof(info)), EAGAIN);
	// Changing the mask to take it makes it readable, and the descriptor
	// is the same.
	sigset_t only_second;
	sigemptyset(&only_second);
	sigaddset(&only_second, SIGUSR2);
	CHECK(signalfd(fd, &only_second, 0) == fd);
	CHECK(ready(fd, POLLIN, 0) & POLLIN);
	CHECK(read(fd, info, sizeof(info[0])) == sizeof(info[0]));
	CHECK(info[0].ssi_signo == SIGUSR2);
	// What the first mask held is not taken now.
	CHECK(kill(getpid(), SIGUSR1) == 0);
	CHECK(ready(fd, POLLIN, 0) == 0);

	// Two signals come in one read, lowest first.
	CHECK(signalfd(fd, &both, 0) == fd);
	CHECK(kill(getpid(), SIGUSR2) == 0);
	memset(info, 0, sizeof(info));
	CHECK(read(fd, info, sizeof(info)) == 2 * (ssize_t)sizeof(info[0]));
	CHECK(info[0].ssi_signo == SIGUSR1);
	CHECK(info[1].ssi_signo == SIGUSR2);
	CHECK_ERROR(read(fd, info, sizeof(info)), EAGAIN);
	// A buffer for one takes one.
	CHECK(kill(getpid(), SIGUSR1) == 0);
	CHECK(kill(getpid(), SIGUSR2) == 0);
	CHECK(read(fd, info, 200) == sizeof(info[0]));
	CHECK(info[0].ssi_signo == SIGUSR1);
	CHECK(read(fd, info, 200) == sizeof(info[0]));
	CHECK(info[0].ssi_signo == SIGUSR2);

	// Errors, in Linux's order: a flag it does not have, a descriptor
	// that is not a signal file, and none.
	CHECK_ERROR(signalfd(-1, &both, 0x40), EINVAL);
	int counter = eventfd(0, 0);
	CHECK(counter >= 0);
	CHECK_ERROR(signalfd(counter, &both, 0), EINVAL);
	close(counter);
	CHECK_ERROR(signalfd(9999, &both, 0), EBADF);
	close(fd);
	sigprocmask(SIG_UNBLOCK, &both, NULL);
}

static void signal_waits(void) {
	sigset_t set;
	sigemptyset(&set);
	sigaddset(&set, SIGUSR1);
	CHECK(sigprocmask(SIG_BLOCK, &set, NULL) == 0);
	int fd = signalfd(-1, &set, 0);
	CHECK(fd >= 0);
	// A blocking read waits for a signal another process sends.
	pid_t parent = getpid();
	pid_t child = check_fork();
	if(child == 0) {
		check_sleep_ms(40);
		CHECK(kill(parent, SIGUSR1) == 0);
		check_exit_child();
	}
	long long start = check_now_ns();
	struct signalfd_siginfo info;
	CHECK(read(fd, &info, sizeof(info)) == sizeof(info));
	long long waited = check_now_ns() - start;
	CHECK(waited >= 20000000LL && waited < 5000000000LL);
	CHECK(info.ssi_signo == SIGUSR1 && info.ssi_pid == (uint32_t)child);
	CHECK_CHILD(child);
	close(fd);
	sigprocmask(SIG_UNBLOCK, &set, NULL);
}

// Forks a child that waits for a byte on `gate`, then exits with `code`.
static pid_t waiting_child(int gate[2], int code) {
	CHECK(pipe(gate) == 0);
	pid_t child = check_fork();
	if(child == 0) {
		close(gate[1]);
		char byte;
		if(read(gate[0], &byte, 1) != 1)
			_exit(99);
		_exit(code);
	}
	close(gate[0]);
	return child;
}

static void processes(void) {
	int gate[2];
	pid_t child = waiting_child(gate, 7);
	CHECK(child > 0);
	int pidfd = open_pidfd(child, 0);
	CHECK(pidfd >= 0);
	CHECK(fcntl(pidfd, F_GETFD) & FD_CLOEXEC);
	// While the child runs it is not readable, and signal 0 finds it.
	CHECK(ready(pidfd, POLLIN, 0) == 0);
	CHECK(signal_pidfd(pidfd, 0, NULL, 0) == 0);
	char byte;
	CHECK_ERROR(read(pidfd, &byte, 1), EINVAL);
	// It becomes readable when the child exits, and stays so.
	CHECK(write(gate[1], "x", 1) == 1);
	CHECK(ready(pidfd, POLLIN, 5000) & POLLIN);
	CHECK(ready(pidfd, POLLIN, 0) & POLLIN);
	// An exited child that nobody waited for is a zombie: signalled still.
	CHECK(signal_pidfd(pidfd, 0, NULL, 0) == 0);
	siginfo_t info;
	memset(&info, 0, sizeof(info));
	CHECK(waitid(P_PIDFD, pidfd, &info, WEXITED) == 0);
	CHECK(info.si_signo == SIGCHLD && info.si_code == CLD_EXITED);
	CHECK(info.si_pid == child && info.si_status == 7);
	close(gate[1]);
	// Once it is waited for there is nobody to signal.
	CHECK_ERROR(signal_pidfd(pidfd, 0, NULL, 0), ESRCH);
	CHECK_ERROR(signal_pidfd(pidfd, SIGTERM, NULL, 0), ESRCH);
	CHECK(ready(pidfd, POLLIN, 0) & POLLIN);
	close(pidfd);
	CHECK_ERROR(open_pidfd(child, 0), ESRCH);
}

static void process_signals(void) {
	// A signal through the file ends a child by it.
	int gate[2];
	pid_t child = waiting_child(gate, 0);
	int pidfd = open_pidfd(child, 0);
	CHECK(pidfd >= 0);
	CHECK(signal_pidfd(pidfd, SIGTERM, NULL, 0) == 0);
	CHECK(ready(pidfd, POLLIN, 5000) & POLLIN);
	siginfo_t info;
	memset(&info, 0, sizeof(info));
	CHECK(waitid(P_PIDFD, pidfd, &info, WEXITED) == 0);
	CHECK(info.si_code == CLD_KILLED && info.si_status == SIGTERM && info.si_pid == child);
	close(gate[1]);
	close(pidfd);

	// A signal with a record brings its value, as sigqueue's does.
	sigset_t set;
	sigemptyset(&set);
	sigaddset(&set, SIGUSR1);
	CHECK(sigprocmask(SIG_BLOCK, &set, NULL) == 0);
	child = check_fork();
	if(child == 0) {
		siginfo_t got;
		if(sigwaitinfo(&set, &got) != SIGUSR1 || got.si_code != SI_QUEUE)
			_exit(98);
		_exit(got.si_value.sival_int);
	}
	sigprocmask(SIG_UNBLOCK, &set, NULL);
	pidfd = open_pidfd(child, 0);
	CHECK(pidfd >= 0);
	memset(&info, 0, sizeof(info));
	info.si_signo = SIGUSR1;
	info.si_code = SI_QUEUE;
	info.si_value.sival_int = 42;
	CHECK(signal_pidfd(pidfd, SIGUSR1, &info, 0) == 0);
	CHECK(ready(pidfd, POLLIN, 5000) & POLLIN);
	int status = 0;
	CHECK(waitpid(child, &status, 0) == child);
	CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 42);
	close(pidfd);

	// A record fakes no other sender's signal to another process.
	int gate2[2];
	child = waiting_child(gate2, 0);
	pidfd = open_pidfd(child, 0);
	CHECK(pidfd >= 0);
	info.si_signo = SIGUSR1;
	info.si_code = SI_USER;
	CHECK_ERROR(signal_pidfd(pidfd, SIGUSR1, &info, 0), EPERM);
	info.si_code = SI_TKILL;
	CHECK_ERROR(signal_pidfd(pidfd, SIGUSR1, &info, 0), EPERM);
	info.si_code = SI_QUEUE;
	info.si_signo = SIGUSR2;
	CHECK_ERROR(signal_pidfd(pidfd, SIGUSR1, &info, 0), EINVAL);
	// And to itself, any code.
	int own = open_pidfd(getpid(), 0);
	CHECK(own >= 0);
	CHECK(signal_pidfd(own, 0, NULL, 0) == 0);
	close(own);
	CHECK(signal_pidfd(pidfd, SIGKILL, NULL, 0) == 0);
	CHECK(waitpid(child, &status, 0) == child);
	CHECK(WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL);
	close(gate2[1]);
	close(pidfd);
}

static void process_waits(void) {
	// A non-blocking file does not wait in waitid, which fails with EAGAIN
	// while no child has ended, unless it is asked for WNOHANG too, which
	// gives an empty record.
	int gate[2];
	pid_t child = waiting_child(gate, 3);
	int pidfd = open_pidfd(child, O_NONBLOCK);
	CHECK(pidfd >= 0);
	CHECK(fcntl(pidfd, F_GETFL) & O_NONBLOCK);
	siginfo_t info;
	CHECK_ERROR(waitid(P_PIDFD, pidfd, &info, WEXITED), EAGAIN);
	memset(&info, 0x55, sizeof(info));
	CHECK(waitid(P_PIDFD, pidfd, &info, WEXITED | WNOHANG) == 0);
	CHECK(info.si_pid == 0);
	CHECK(write(gate[1], "x", 1) == 1);
	CHECK(ready(pidfd, POLLIN, 5000) & POLLIN);
	memset(&info, 0, sizeof(info));
	CHECK(waitid(P_PIDFD, pidfd, &info, WEXITED) == 0);
	CHECK(info.si_pid == child && info.si_status == 3);
	close(gate[1]);
	close(pidfd);

	// A blocking one waits for the child to end.
	pid_t other = waiting_child(gate, 5);
	int blocking = open_pidfd(other, 0);
	CHECK(blocking >= 0);
	pid_t waker = check_fork();
	if(waker == 0) {
		check_sleep_ms(40);
		CHECK(write(gate[1], "x", 1) == 1);
		check_exit_child();
	}
	long long start = check_now_ns();
	memset(&info, 0, sizeof(info));
	CHECK(waitid(P_PIDFD, blocking, &info, WEXITED) == 0);
	long long waited = check_now_ns() - start;
	CHECK(waited >= 20000000LL && waited < 5000000000LL);
	CHECK(info.si_pid == other && info.si_status == 5);
	CHECK_CHILD(waker);
	close(gate[1]);
	close(blocking);
}

static void process_errors(void) {
	// Flags it does not have, and no process.
	CHECK_ERROR(open_pidfd(getpid(), 0x40000000), EINVAL);
	CHECK_ERROR(open_pidfd(0, 0), EINVAL);
	CHECK_ERROR(open_pidfd(-1, 0), EINVAL);
	// Descriptors that are not process files, and none.
	int counter = eventfd(0, 0);
	CHECK(counter >= 0);
	CHECK_ERROR(signal_pidfd(counter, 0, NULL, 0), EBADF);
	siginfo_t info;
	CHECK_ERROR(waitid(P_PIDFD, counter, &info, WEXITED), EBADF);
	CHECK_ERROR(signal_pidfd(9999, 0, NULL, 0), EBADF);
	CHECK_ERROR(waitid(P_PIDFD, 9999, &info, WEXITED), EBADF);
	close(counter);
	int own = open_pidfd(getpid(), 0);
	CHECK(own >= 0);
	CHECK_ERROR(signal_pidfd(own, 0, NULL, 0x100), EINVAL);
	CHECK_ERROR(signal_pidfd(own, -1, NULL, 0), EINVAL);
	CHECK_ERROR(signal_pidfd(own, 4096, NULL, 0), EINVAL);
	// A process file takes no writes.
	CHECK_ERROR(write(own, "x", 1), EINVAL);
	close(own);
}

int main(void) {
	check_start("kernelfiles");
	RUN(counters);
	RUN(counter_waits);
	RUN(timers);
	RUN(timer_errors);
	RUN(signals);
	RUN(signal_masks);
	RUN(signal_waits);
	RUN(processes);
	RUN(process_signals);
	RUN(process_waits);
	RUN(process_errors);
	return check_end();
}
