// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// epoll(7) (ADR 0612): the calls' errors, level-triggered, edge-triggered
// and oneshot registrations, EPOLL_CTL_MOD, EPOLLRDHUP on a socket,
// EPOLLEXCLUSIVE across two instances, epoll_pwait's signal mask,
// timeouts, a registration that outlives its descriptor, an instance
// shared by fork, and a thousand pipes of which one is ready. It passes
// on Linux too: what it checks is Linux's behaviour.

#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <sys/epoll.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <sys/stat.h>

#include "check.h"

// A wait for what is there or soon will be, in milliseconds: five
// seconds at most, so that a lost event fails a check where it would hang
// the test.
#define PATIENCE 5000

// Registers `fd` with `epfd` for `events`, with `data` as its data.
static int add(int epfd, int fd, uint32_t events, uint64_t data) {
	struct epoll_event event = {.events = events, .data = {.u64 = data}};
	return epoll_ctl(epfd, EPOLL_CTL_ADD, fd, &event);
}

// Changes the registration of `fd` with `epfd` to `events` and `data`.
static int modify(int epfd, int fd, uint32_t events, uint64_t data) {
	struct epoll_event event = {.events = events, .data = {.u64 = data}};
	return epoll_ctl(epfd, EPOLL_CTL_MOD, fd, &event);
}

// What `epfd` has ready now, without waiting: how many events, the first
// in `event`.
static int look(int epfd, struct epoll_event *event) {
	return epoll_wait(epfd, event, 1, 0);
}

// Milliseconds on the monotonic clock.
static long long now_ms(void) {
	struct timespec now;
	clock_gettime(CLOCK_MONOTONIC, &now);
	return (long long)now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

static void errors(void) {
	CHECK_ERROR(epoll_create(0), EINVAL);
	CHECK_ERROR(epoll_create(-1), EINVAL);
	CHECK_ERROR(epoll_create1(1), EINVAL);
	int epfd = epoll_create1(EPOLL_CLOEXEC);
	CHECK(epfd >= 0);
	CHECK(fcntl(epfd, F_GETFD) == FD_CLOEXEC);
	int fds[2];
	CHECK(pipe(fds) == 0);
	// A regular file is always ready: epoll refuses it.
	int file = open("/tmp/posix-epoll-file", O_RDWR | O_CREAT | O_TRUNC, 0600);
	CHECK(file >= 0);
	CHECK_ERROR(add(epfd, file, EPOLLIN, 0), EPERM);
	CHECK_ERROR(add(epfd, epfd, EPOLLIN, 0), EINVAL);
	CHECK_ERROR(add(fds[1], fds[0], EPOLLIN, 0), EINVAL);
	CHECK_ERROR(add(epfd, 999, EPOLLIN, 0), EBADF);
	CHECK_ERROR(add(-1, fds[0], EPOLLIN, 0), EBADF);
	CHECK(add(epfd, fds[0], EPOLLIN, 0) == 0);
	CHECK_ERROR(add(epfd, fds[0], EPOLLIN, 0), EEXIST);
	CHECK_ERROR(modify(epfd, fds[1], EPOLLOUT, 0), ENOENT);
	CHECK_ERROR(epoll_ctl(epfd, EPOLL_CTL_DEL, fds[1], NULL), ENOENT);
	struct epoll_event event = {.events = EPOLLIN};
	CHECK_ERROR(epoll_ctl(epfd, 99, fds[0], &event), EINVAL);
	// EPOLLEXCLUSIVE is for an add of plain events only.
	CHECK_ERROR(add(epfd, fds[1], EPOLLOUT | EPOLLEXCLUSIVE | EPOLLONESHOT, 0), EINVAL);
	CHECK_ERROR(add(epfd, fds[1], EPOLLOUT | EPOLLEXCLUSIVE | EPOLLRDHUP, 0), EINVAL);
	CHECK_ERROR(modify(epfd, fds[0], EPOLLIN | EPOLLEXCLUSIVE, 0), EINVAL);
	CHECK(add(epfd, fds[1], EPOLLOUT | EPOLLEXCLUSIVE, 0) == 0);
	CHECK_ERROR(modify(epfd, fds[1], EPOLLOUT, 0), EINVAL);
	CHECK(epoll_ctl(epfd, EPOLL_CTL_DEL, fds[1], NULL) == 0);
	CHECK(epoll_ctl(epfd, EPOLL_CTL_DEL, fds[0], NULL) == 0);
	CHECK_ERROR(epoll_ctl(epfd, EPOLL_CTL_DEL, fds[0], NULL), ENOENT);
	// The waits.
	struct epoll_event events[4];
	CHECK_ERROR(epoll_wait(epfd, events, 0, 0), EINVAL);
	// A count from a variable the compiler cannot see is negative.
	volatile int negative = -1;
	CHECK_ERROR(epoll_wait(epfd, events, negative, 0), EINVAL);
	CHECK_ERROR(epoll_wait(fds[0], events, 4, 0), EINVAL);
	CHECK_ERROR(epoll_wait(-1, events, 4, 0), EBADF);
	CHECK(epoll_wait(epfd, events, 4, 0) == 0);
	close(file);
	unlink("/tmp/posix-epoll-file");
	close(fds[0]);
	close(fds[1]);
	close(epfd);
}

static void level(void) {
	int epfd = epoll_create1(0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(add(epfd, fds[0], EPOLLIN, 0x1122334455667788ULL) == 0);
	struct epoll_event event;
	CHECK(look(epfd, &event) == 0);
	CHECK(write(fds[1], "ab", 2) == 2);
	// Reported for as long as it stays readable, with the data it was
	// given.
	for(int i = 0; i < 3; i++) {
		CHECK(epoll_wait(epfd, &event, 1, PATIENCE) == 1);
		CHECK(event.events == EPOLLIN);
		CHECK(event.data.u64 == 0x1122334455667788ULL);
	}
	char buffer[2];
	CHECK(read(fds[0], buffer, 2) == 2);
	CHECK(look(epfd, &event) == 0);
	// The write end: writable, then an error once no one reads.
	CHECK(add(epfd, fds[1], EPOLLOUT, 2) == 0);
	CHECK(look(epfd, &event) == 1);
	CHECK(event.events == EPOLLOUT && event.data.u64 == 2);
	CHECK(epoll_ctl(epfd, EPOLL_CTL_DEL, fds[0], NULL) == 0);
	close(fds[0]);
	CHECK(look(epfd, &event) == 1);
	CHECK(event.events == (EPOLLOUT | EPOLLERR));
	close(fds[1]);
	close(epfd);
}

static void edge(void) {
	int epfd = epoll_create1(0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(add(epfd, fds[0], EPOLLIN | EPOLLET, 7) == 0);
	struct epoll_event event;
	CHECK(write(fds[1], "a", 1) == 1);
	CHECK(epoll_wait(epfd, &event, 1, PATIENCE) == 1);
	CHECK(event.events == EPOLLIN && event.data.u64 == 7);
	// Once for each change, though it is still readable.
	CHECK(look(epfd, &event) == 0);
	CHECK(write(fds[1], "b", 1) == 1);
	CHECK(epoll_wait(epfd, &event, 1, PATIENCE) == 1);
	CHECK(look(epfd, &event) == 0);
	// EPOLL_CTL_MOD makes it look again.
	CHECK(modify(epfd, fds[0], EPOLLIN | EPOLLET, 8) == 0);
	CHECK(look(epfd, &event) == 1);
	CHECK(event.data.u64 == 8);
	close(fds[0]);
	close(fds[1]);
	close(epfd);
}

static void oneshot(void) {
	int epfd = epoll_create1(0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(add(epfd, fds[0], EPOLLIN | EPOLLONESHOT, 3) == 0);
	struct epoll_event event;
	CHECK(write(fds[1], "a", 1) == 1);
	CHECK(epoll_wait(epfd, &event, 1, PATIENCE) == 1);
	CHECK(event.events == EPOLLIN);
	// Disabled once reported, not removed: a change reports nothing, an
	// add fails, and EPOLL_CTL_MOD arms it again.
	CHECK(write(fds[1], "b", 1) == 1);
	CHECK(look(epfd, &event) == 0);
	CHECK_ERROR(add(epfd, fds[0], EPOLLIN, 0), EEXIST);
	CHECK(modify(epfd, fds[0], EPOLLIN | EPOLLONESHOT, 4) == 0);
	CHECK(look(epfd, &event) == 1);
	CHECK(event.data.u64 == 4);
	CHECK(look(epfd, &event) == 0);
	close(fds[0]);
	close(fds[1]);
	close(epfd);
}

static void read_hang_up(void) {
	int epfd = epoll_create1(0);
	int pair[2];
	CHECK(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
	CHECK(add(epfd, pair[0], EPOLLIN | EPOLLRDHUP, 0) == 0);
	struct epoll_event event;
	CHECK(look(epfd, &event) == 0);
	// The peer shuts its writing side: the end of what it sends.
	CHECK(shutdown(pair[1], SHUT_WR) == 0);
	CHECK(epoll_wait(epfd, &event, 1, PATIENCE) == 1);
	CHECK(event.events == (EPOLLIN | EPOLLRDHUP));
	// Not asked for, it is not reported, but the end still reads.
	CHECK(modify(epfd, pair[0], EPOLLIN, 0) == 0);
	CHECK(look(epfd, &event) == 1);
	CHECK(event.events == EPOLLIN);
	// Gone altogether: hung up, whether asked for or not.
	close(pair[1]);
	CHECK(modify(epfd, pair[0], EPOLLRDHUP, 0) == 0);
	CHECK(look(epfd, &event) == 1);
	CHECK(event.events == (EPOLLRDHUP | EPOLLHUP));
	close(pair[0]);
	close(epfd);
}

// A thread that waits on an epoll instance.
struct waiter {
	int epfd;
	int found;
};

static void *wait_on(void *argument) {
	struct waiter *waiter = argument;
	struct epoll_event event;
	waiter->found = epoll_wait(waiter->epfd, &event, 1, PATIENCE);
	return NULL;
}

static void exclusive(void) {
	// Two instances watch one pipe, each exclusively, the first one
	// registered first. A change wakes a waiter of the first and stops
	// there, so the second never hears of it. The waiter is given time to
	// be waiting before the write; if it was not yet, both hear of it,
	// and the round is made again.
	int rounds = 0, alone = 0;
	for(; rounds < 5 && !alone; rounds++) {
		int first = epoll_create1(0), second = epoll_create1(0);
		int fds[2];
		CHECK(pipe(fds) == 0);
		CHECK(add(first, fds[0], EPOLLIN | EPOLLEXCLUSIVE, 1) == 0);
		CHECK(add(second, fds[0], EPOLLIN | EPOLLEXCLUSIVE, 2) == 0);
		struct waiter waiter = {.epfd = first, .found = -1};
		pthread_t thread;
		CHECK(pthread_create(&thread, NULL, wait_on, &waiter) == 0);
		usleep(100000 << rounds);
		CHECK(write(fds[1], "a", 1) == 1);
		CHECK(pthread_join(thread, NULL) == 0);
		CHECK(waiter.found == 1);
		struct epoll_event event;
		alone = look(second, &event) == 0;
		close(fds[0]);
		close(fds[1]);
		close(first);
		close(second);
	}
	CHECK(alone);
	// With nobody waiting, each instance hears of the change.
	int first = epoll_create1(0), second = epoll_create1(0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(add(first, fds[0], EPOLLIN | EPOLLEXCLUSIVE, 1) == 0);
	CHECK(add(second, fds[0], EPOLLIN | EPOLLEXCLUSIVE, 2) == 0);
	CHECK(write(fds[1], "a", 1) == 1);
	struct epoll_event event;
	CHECK(look(first, &event) == 1 && event.data.u64 == 1);
	CHECK(look(second, &event) == 1 && event.data.u64 == 2);
	close(fds[0]);
	close(fds[1]);
	close(first);
	close(second);
}

static volatile sig_atomic_t caught;

static void on_signal(int signal) {
	(void)signal;
	caught = 1;
}

// A thread that sends SIGUSR1 to `argument`, a thread, after 50 ms.
static void *signal_later(void *argument) {
	usleep(50000);
	pthread_kill(*(pthread_t *)argument, SIGUSR1);
	return NULL;
}

static void signal_mask(void) {
	struct sigaction action = {.sa_handler = on_signal, .sa_flags = SA_RESTART};
	sigemptyset(&action.sa_mask);
	CHECK(sigaction(SIGUSR1, &action, NULL) == 0);
	sigset_t blocked, old, during;
	sigemptyset(&blocked);
	sigaddset(&blocked, SIGUSR1);
	CHECK(pthread_sigmask(SIG_BLOCK, &blocked, &old) == 0);
	sigemptyset(&during);
	int epfd = epoll_create1(0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(add(epfd, fds[0], EPOLLIN, 0) == 0);
	pthread_t self = pthread_self(), thread;
	CHECK(pthread_create(&thread, NULL, signal_later, &self) == 0);
	struct epoll_event event;
	caught = 0;
	// The signal is let through while it waits, and ends the wait with
	// EINTR, SA_RESTART or not.
	CHECK_ERROR(epoll_pwait(epfd, &event, 1, PATIENCE, &during), EINTR);
	CHECK(caught == 1);
	CHECK(pthread_join(thread, NULL) == 0);
	// The mask it had is back.
	sigset_t now;
	CHECK(pthread_sigmask(SIG_SETMASK, NULL, &now) == 0);
	CHECK(sigismember(&now, SIGUSR1) == 1);
	CHECK(pthread_sigmask(SIG_SETMASK, &old, NULL) == 0);
	signal(SIGUSR1, SIG_DFL);
	close(fds[0]);
	close(fds[1]);
	close(epfd);
}

static void timeouts(void) {
	int epfd = epoll_create1(0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(add(epfd, fds[0], EPOLLIN, 0) == 0);
	struct epoll_event event;
	long long start = now_ms();
	CHECK(epoll_wait(epfd, &event, 1, 0) == 0);
	CHECK(now_ms() - start < 1000);
	start = now_ms();
	CHECK(epoll_wait(epfd, &event, 1, 50) == 0);
	long long waited = now_ms() - start;
	CHECK(waited >= 45 && waited < 2000);
	close(fds[0]);
	close(fds[1]);
	close(epfd);
}

static void lifetime(void) {
	// A registration is of the descriptor's description: it outlives the
	// descriptor while a copy keeps the description open.
	int epfd = epoll_create1(0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(add(epfd, fds[0], EPOLLIN, 5) == 0);
	int copy = dup(fds[0]);
	CHECK(copy >= 0);
	close(fds[0]);
	CHECK(write(fds[1], "a", 1) == 1);
	struct epoll_event event;
	CHECK(epoll_wait(epfd, &event, 1, PATIENCE) == 1);
	CHECK(event.data.u64 == 5);
	// The copy is another descriptor: it is not registered.
	CHECK_ERROR(epoll_ctl(epfd, EPOLL_CTL_DEL, copy, NULL), ENOENT);
	// Gone with the description's last descriptor.
	close(copy);
	CHECK(look(epfd, &event) == 0);
	close(fds[1]);
	close(epfd);
}

static void forked(void) {
	// A child shares the instance: what it makes ready, its parent hears
	// of, and it may wait on the instance too.
	int epfd = epoll_create1(0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(add(epfd, fds[0], EPOLLIN, 9) == 0);
	pid_t child = fork();
	if(child == 0) {
		struct epoll_event event;
		int ok = write(fds[1], "a", 1) == 1;
		ok = ok && epoll_wait(epfd, &event, 1, PATIENCE) == 1 && event.data.u64 == 9;
		_exit(ok ? 0 : 1);
	}
	CHECK(child > 0);
	struct epoll_event event;
	CHECK(epoll_wait(epfd, &event, 1, PATIENCE) == 1);
	CHECK(event.data.u64 == 9);
	int status;
	CHECK(waitpid(child, &status, 0) == child);
	CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
	close(fds[0]);
	close(fds[1]);
	close(epfd);
}

static void many(void) {
	// A thousand pipes, or as many as the limit of open files allows, of
	// which one is ready: found without a look at the others.
	struct rlimit limit;
	CHECK(getrlimit(RLIMIT_NOFILE, &limit) == 0);
	rlim_t want = 2100;
	if(limit.rlim_cur < want && limit.rlim_max != RLIM_INFINITY && limit.rlim_max < want)
		want = limit.rlim_max;
	if(limit.rlim_cur < want) {
		limit.rlim_cur = want;
		CHECK(setrlimit(RLIMIT_NOFILE, &limit) == 0);
	}
	int count = (int)((want - 50) / 2);
	if(count > 1000)
		count = 1000;
	int epfd = epoll_create1(0);
	static int ends[1000][2];
	for(int i = 0; i < count; i++) {
		CHECK(pipe(ends[i]) == 0);
		CHECK(add(epfd, ends[i][0], EPOLLIN, (uint64_t)i) == 0);
	}
	struct epoll_event events[8];
	CHECK(epoll_wait(epfd, events, 8, 0) == 0);
	int chosen = count * 2 / 3;
	CHECK(write(ends[chosen][1], "a", 1) == 1);
	CHECK(epoll_wait(epfd, events, 8, PATIENCE) == 1);
	CHECK(events[0].data.u64 == (uint64_t)chosen);
	for(int i = 0; i < count; i++) {
		close(ends[i][0]);
		close(ends[i][1]);
	}
	close(epfd);
}

int main(void) {
	check_start("epoll");
	RUN(errors);
	RUN(level);
	RUN(edge);
	RUN(oneshot);
	RUN(read_hang_up);
	RUN(exclusive);
	RUN(signal_mask);
	RUN(timeouts);
	RUN(lifetime);
	RUN(forked);
	RUN(many);
	return check_end();
}
