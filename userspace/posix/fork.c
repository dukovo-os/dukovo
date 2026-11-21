// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// fork and wait: the child is a copy whose writes the parent does not
// see, except in shared memory, which both see and whose futexes wake
// across them, and the parent waits for its children in every way
// waitpid has. The child's descriptors are capabilities derived from the
// parent's, which the parent can revoke while the child uses them. A
// signal that reaches a child as it starts is the child's to take.

#include <fcntl.h>
#include <pthread.h>
#include <radix/radix.h>
#include <sched.h>
#include <semaphore.h>
#include <stdatomic.h>
#include <stdint.h>
#include <sys/mman.h>
#include <sys/stat.h>

#include "check.h"

static int global = 1;

// The child gets a copy of the parent's memory: what it writes, the
// parent does not see.
static void copies(void) {
	int local = 2;
	int *heap = malloc(sizeof(int));
	*heap = 3;
	size_t big = 4 << 20;
	unsigned char *pattern = malloc(big);
	CHECK(pattern != NULL);
	for(size_t i = 0; i < big; i++)
		pattern[i] = (unsigned char)(i * 7);
	pid_t parent = getpid();
	pid_t pid = check_fork();
	if(pid == 0) {
		CHECK(getppid() == parent);
		CHECK(getpid() != parent);
		CHECK(global == 1 && local == 2 && *heap == 3);
		size_t wrong = 0;
		for(size_t i = 0; i < big; i++)
			wrong += pattern[i] != (unsigned char)(i * 7);
		CHECK(wrong == 0);
		global = 10;
		local = 20;
		*heap = 30;
		memset(pattern, 0xee, big);
		CHECK(global == 10 && local == 20 && *heap == 30 && pattern[big - 1] == 0xee);
		check_exit_child();
	}
	CHECK(pid > 0);
	CHECK_CHILD(pid);
	CHECK(global == 1 && local == 2 && *heap == 3);
	size_t wrong = 0;
	for(size_t i = 0; i < big; i++)
		wrong += pattern[i] != (unsigned char)(i * 7);
	CHECK(wrong == 0);
	free(pattern);
	free(heap);
}

// Shared memory stays shared: a MAP_SHARED anonymous mapping is the same
// memory in both.
static void shares(void) {
	volatile int *shared =
			mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
	CHECK(shared != MAP_FAILED);
	if(shared == MAP_FAILED)
		return;
	*shared = 1;
	pid_t pid = check_fork();
	if(pid == 0) {
		*shared = 2;
		check_exit_child();
	}
	CHECK_CHILD(pid);
	CHECK(*shared == 2);
	munmap((void *)shared, 4096);
}

// A page of shared memory first touched after the fork is the same page
// on both sides, whichever side touches it first.
static void shares_new_pages(void) {
	volatile int *shared =
			mmap(NULL, 3 * 4096, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
	CHECK(shared != MAP_FAILED);
	if(shared == MAP_FAILED)
		return;
	int go[2];
	CHECK(pipe(go) == 0);
	// One child writes the second page and ends; another reads the third
	// once the parent has written it.
	pid_t writer = check_fork();
	if(writer == 0) {
		shared[1024] = 41;
		check_exit_child();
	}
	pid_t reader = check_fork();
	if(reader == 0) {
		close(go[1]);
		char byte;
		CHECK(read(go[0], &byte, 1) == 1);
		CHECK(shared[2048] == 42);
		CHECK(shared[1024] == 41);
		check_exit_child();
	}
	close(go[0]);
	CHECK_CHILD(writer);
	CHECK(shared[1024] == 41);
	shared[2048] = 42;
	CHECK(write(go[1], "x", 1) == 1);
	close(go[1]);
	CHECK_CHILD(reader);
	CHECK(shared[0] == 0);
	munmap((void *)shared, 3 * 4096);
}

// Two semaphores in shared memory (pshared), passed back and forth with
// another process: each wake reaches a waiter in the other process.
struct turns {
	sem_t ask, answer;
};

static int turn(sem_t *sem) {
	struct timespec until;
	clock_gettime(CLOCK_REALTIME, &until);
	until.tv_sec += 10;
	int result;
	while((result = sem_timedwait(sem, &until)) != 0 && errno == EINTR) {}
	return result;
}

static void shared_semaphores(void) {
	struct turns *turns =
			mmap(NULL, sizeof(*turns), PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
	CHECK(turns != MAP_FAILED);
	if(turns == MAP_FAILED)
		return;
	CHECK(sem_init(&turns->ask, 1, 0) == 0);
	CHECK(sem_init(&turns->answer, 1, 0) == 0);
	enum { ROUNDS = 200 };
	pid_t pid = check_fork();
	if(pid == 0) {
		for(int i = 0; i < ROUNDS; i++) {
			if(turn(&turns->ask) != 0) {
				CHECK(!"the parent's post woke this process");
				break;
			}
			sem_post(&turns->answer);
		}
		check_exit_child();
	}
	int answered = 0;
	for(int i = 0; i < ROUNDS; i++) {
		sem_post(&turns->ask);
		if(turn(&turns->answer) != 0)
			break;
		answered++;
	}
	CHECK(answered == ROUNDS);
	CHECK_CHILD(pid);
	sem_destroy(&turns->ask);
	sem_destroy(&turns->answer);
	munmap(turns, sizeof(*turns));
}

// A mutex in shared memory (PTHREAD_PROCESS_SHARED) keeps two processes'
// increments of a counter apart, sleeping and waking across them.
struct counted {
	pthread_mutex_t lock;
	long count;
};

static void shared_mutex(void) {
	struct counted *counted =
			mmap(NULL, sizeof(*counted), PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
	CHECK(counted != MAP_FAILED);
	if(counted == MAP_FAILED)
		return;
	pthread_mutexattr_t attr;
	CHECK(pthread_mutexattr_init(&attr) == 0);
	CHECK(pthread_mutexattr_setpshared(&attr, PTHREAD_PROCESS_SHARED) == 0);
	CHECK(pthread_mutex_init(&counted->lock, &attr) == 0);
	pthread_mutexattr_destroy(&attr);
	enum { EACH = 20000 };
	pid_t pid = check_fork();
	for(int i = 0; i < EACH; i++) {
		pthread_mutex_lock(&counted->lock);
		long seen = counted->count;
		// Long enough inside for the other process to find it locked.
		if(i % 64 == 0)
			sched_yield();
		counted->count = seen + 1;
		pthread_mutex_unlock(&counted->lock);
	}
	if(pid == 0)
		check_exit_child();
	CHECK_CHILD(pid);
	CHECK(counted->count == 2 * EACH);
	pthread_mutex_destroy(&counted->lock);
	munmap(counted, sizeof(*counted));
}

// Exit codes and wait statuses.
static void statuses(void) {
	for(int code = 0; code < 256; code += 51) {
		pid_t pid = check_fork();
		if(pid == 0)
			_exit(code);
		int status = check_wait(pid);
		CHECK(WIFEXITED(status) && WEXITSTATUS(status) == code);
	}
	// exit() runs the atexit handlers and flushes stdio; _exit does not.
	int fds[2];
	CHECK(pipe(fds) == 0);
	pid_t pid = check_fork();
	if(pid == 0) {
		close(fds[0]);
		dup2(fds[1], STDOUT_FILENO);
		setvbuf(stdout, NULL, _IOFBF, 4096);
		fputs("flushed at exit", stdout);
		exit(7);
	}
	close(fds[1]);
	char buffer[64] = {0};
	ssize_t n = 0, got;
	while((got = read(fds[0], buffer + n, sizeof(buffer) - 1 - n)) > 0)
		n += got;
	close(fds[0]);
	CHECK(strcmp(buffer, "flushed at exit") == 0);
	int status = check_wait(pid);
	CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 7);
}

// waitpid for any child, for none, and without waiting.
static void waits(void) {
	enum { CHILDREN = 8 };
	pid_t pids[CHILDREN];
	for(int i = 0; i < CHILDREN; i++) {
		pids[i] = check_fork();
		if(pids[i] == 0)
			_exit(100 + i);
	}
	int seen = 0;
	for(int i = 0; i < CHILDREN; i++) {
		int status;
		pid_t got = waitpid(-1, &status, 0);
		for(int j = 0; j < CHILDREN; j++) {
			if(got == pids[j] && WIFEXITED(status) && WEXITSTATUS(status) == 100 + j)
				seen |= 1 << j;
		}
	}
	CHECK(seen == (1 << CHILDREN) - 1);
	CHECK_ERROR(waitpid(-1, NULL, 0), ECHILD);
	CHECK_ERROR(waitpid(-1, NULL, WNOHANG), ECHILD);
	CHECK_ERROR(wait(NULL), ECHILD);
	// A child still running: WNOHANG returns 0 until it ends.
	int fds[2];
	CHECK(pipe(fds) == 0);
	pid_t pid = check_fork();
	if(pid == 0) {
		close(fds[1]);
		char byte;
		_exit(read(fds[0], &byte, 1) == 1 && byte == 'x' ? 0 : 1);
	}
	close(fds[0]);
	int status;
	CHECK(waitpid(pid, &status, WNOHANG) == 0);
	CHECK(write(fds[1], "x", 1) == 1);
	close(fds[1]);
	CHECK_CHILD(pid);
	// A child that is not ours.
	CHECK_ERROR(waitpid(getpid(), NULL, 0), ECHILD);
	// waitid sees it too, and WNOWAIT leaves it to be waited for again.
	pid = check_fork();
	if(pid == 0)
		_exit(5);
	siginfo_t info;
	memset(&info, 0, sizeof(info));
	CHECK(waitid(P_PID, pid, &info, WEXITED | WNOWAIT) == 0);
	CHECK(info.si_pid == pid && info.si_code == CLD_EXITED && info.si_status == 5);
	CHECK(waitpid(pid, &status, 0) == pid && WEXITSTATUS(status) == 5);
	// waitid without WEXITED passes over children that ended, as on
	// Linux: WSTOPPED alone hears of a stop, and the ended child stays.
	pid = check_fork();
	if(pid == 0)
		_exit(6);
	pid_t stopper = check_fork();
	if(stopper == 0) {
		raise(SIGSTOP);
		_exit(0);
	}
	memset(&info, 0, sizeof(info));
	CHECK(waitid(P_ALL, 0, &info, WSTOPPED) == 0);
	CHECK(info.si_pid == stopper && info.si_code == CLD_STOPPED && info.si_status == SIGSTOP);
	CHECK(waitid(P_PID, pid, &info, WSTOPPED | WNOHANG) == 0 && info.si_pid == 0);
	CHECK_ERROR(waitid(P_ALL, 0, &info, WNOHANG), EINVAL);
	CHECK(kill(stopper, SIGCONT) == 0);
	CHECK_CHILD(stopper);
	CHECK(waitpid(pid, &status, 0) == pid && WEXITSTATUS(status) == 6);
}

// Descriptors are copied, and a copy shares its description's offset.
static void descriptors(void) {
	int fd = open("/tmp/fork-offset", O_RDWR | O_CREAT | O_TRUNC, 0644);
	CHECK(fd >= 0);
	pid_t pid = check_fork();
	if(pid == 0) {
		CHECK(write(fd, "child", 5) == 5);
		check_exit_child();
	}
	CHECK_CHILD(pid);
	CHECK(lseek(fd, 0, SEEK_CUR) == 5);
	CHECK(write(fd, "parent", 6) == 6);
	char buffer[16] = {0};
	CHECK(pread(fd, buffer, sizeof(buffer) - 1, 0) == 11);
	CHECK(strcmp(buffer, "childparent") == 0);
	close(fd);
	unlink("/tmp/fork-offset");
}

// A child's descriptor is a capability derived from its parent's, so the
// parent's revoke takes it from the child, even while the child's calls
// use it. The child has one thread, so each of its calls on the
// directory borrows it without counting a reference, and the revoke
// waits for those calls to count theirs before it deletes (ADR 0414): the
// child's opens then fail, and the parent's own descriptor stays.
static void revoked_while_used(void) {
	CHECK(mkdir("/tmp/fork-revoke", 0755) == 0);
	int made = open("/tmp/fork-revoke/f", O_WRONLY | O_CREAT | O_EXCL, 0644);
	CHECK(made >= 0);
	close(made);
	int dir = open("/tmp/fork-revoke", O_RDONLY | O_DIRECTORY);
	CHECK(dir >= 0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	pid_t pid = check_fork();
	if(pid == 0) {
		close(fds[0]);
		// Opens beneath it until it goes, for 60 s at most.
		struct timespec start, now;
		clock_gettime(CLOCK_MONOTONIC, &start);
		for(long opened = 1;; opened++) {
			int fd = openat(dir, "f", O_RDONLY);
			if(fd < 0)
				_exit(opened > 100 && errno == EBADF ? 0 : 1);
			close(fd);
			if(opened == 100)
				CHECK(write(fds[1], "r", 1) == 1);
			clock_gettime(CLOCK_MONOTONIC, &now);
			if(now.tv_sec - start.tv_sec > 60)
				_exit(2);
		}
	}
	close(fds[1]);
	char ready;
	CHECK(read(fds[0], &ready, 1) == 1);
	CHECK(radix_capability_revoke(RADIX_STARTUP_SLOT_FD_BASE + (uint32_t)dir) == 0);
	CHECK_CHILD(pid);
	int fd = openat(dir, "f", O_RDONLY);
	CHECK(fd >= 0);
	close(fd);
	close(fds[0]);
	close(dir);
	CHECK(unlink("/tmp/fork-revoke/f") == 0 && rmdir("/tmp/fork-revoke") == 0);
}

// A child whose parent ends loses its parent, and its process group and
// session stay.
static void orphans(void) {
	int fds[2];
	CHECK(pipe(fds) == 0);
	pid_t pid = check_fork();
	if(pid == 0) {
		pid_t middle = getpid();
		pid_t grandchild = fork();
		if(grandchild == 0) {
			close(fds[0]);
			// Until its parent has gone.
			while(getppid() == middle)
				check_sleep_ms(5);
			pid_t ids[3] = {getppid(), getpgrp(), getsid(0)};
			write(fds[1], ids, sizeof(ids));
			_exit(0);
		}
		_exit(grandchild > 0 ? 0 : 1);
	}
	close(fds[1]);
	CHECK_CHILD(pid);
	pid_t ids[3];
	CHECK(read(fds[0], ids, sizeof(ids)) == sizeof(ids));
	CHECK(ids[0] != pid);
	CHECK(ids[1] == getpgrp() && ids[2] == getsid(0));
	close(fds[0]);
}

// What the handler of a round's child found, in memory the round's parent
// sees: getpid() there, and what pthread_kill(pthread_self(), 0) gave.
struct child_found {
	_Atomic pid_t pid;
	_Atomic int self;
};
static struct child_found *found;

// The handler of a child that a signal reaches as it starts: notes what
// it finds itself to be, then ends its process as the signal's default
// would, as PHP's handlers do.
static void end_as_default(int number) {
	atomic_store(&found->pid, getpid());
	atomic_store(&found->self, pthread_kill(pthread_self(), 0));
	struct sigaction action;
	memset(&action, 0, sizeof(action));
	action.sa_handler = SIG_DFL;
	sigaction(number, &action, NULL);
	sigset_t set;
	sigemptyset(&set);
	sigaddset(&set, number);
	sigprocmask(SIG_UNBLOCK, &set, NULL);
	kill(getpid(), number);
}

// A round of `signal_at_start`, in a child of the test: forks a child
// that spins, as PHP's pcntl test does, and sends it SIGTERM at once.
// Exits 0 if the child ended by it, its handler having found the child's
// own numbers, and otherwise with what went wrong.
static void signal_round(void) {
	struct sigaction action;
	memset(&action, 0, sizeof(action));
	action.sa_handler = end_as_default;
	sigemptyset(&action.sa_mask);
	if(sigaction(SIGTERM, &action, NULL) != 0)
		_exit(10);
	atomic_store(&found->pid, 0);
	atomic_store(&found->self, -1);
	pid_t pid = fork();
	if(pid == 0) {
		long long deadline = check_now_ns() + 2000000000LL;
		while(check_now_ns() < deadline) {
		}
		_exit(3);
	}
	if(pid < 0 || kill(pid, SIGTERM) != 0)
		_exit(11);
	int status;
	pid_t got;
	do {
		got = waitpid(pid, &status, 0);
	} while(got < 0 && errno == EINTR);
	if(got != pid || !WIFSIGNALED(status) || WTERMSIG(status) != SIGTERM)
		_exit(12);
	if(atomic_load(&found->pid) != pid)
		_exit(13);
	if(atomic_load(&found->self) != 0)
		_exit(14);
	_exit(0);
}

// A signal that reaches a child as it starts, before it has run a line of
// its own, is taken by the child as itself: its handler finds the child's
// process and thread numbers, so one that ends its process as the
// signal's default would, with kill(getpid()), as PHP's handlers do, ends
// the child and not the parent (PHP's ext/pcntl/tests/001.phpt). In most
// rounds the signal comes before the child has run; half run on one CPU
// and half on all, which order parent and child differently. Each round's
// parent is a child of the test, so a signal that went astray ends that
// parent, not the test, and the round's child spins for 2 s at most.
static void signal_at_start(void) {
	found = mmap(NULL, sizeof(*found), PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
	CHECK(found != MAP_FAILED);
	if(found == MAP_FAILED)
		return;
	cpu_set_t all, one;
	CPU_ZERO(&all);
	CHECK(sched_getaffinity(0, sizeof(all), &all) == 0);
	int first = 0;
	while(first < CPU_SETSIZE - 1 && !CPU_ISSET(first, &all))
		first++;
	CPU_ZERO(&one);
	CPU_SET(first, &one);
	for(int round = 0; round < 40; round++) {
		pid_t parent = check_fork();
		if(parent == 0) {
			if(round % 2 && sched_setaffinity(0, sizeof(one), &one) != 0)
				_exit(15);
			signal_round();
		}
		int status = check_wait(parent);
		if(!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
			printf("%s: round %d's parent ended with status %#x\n", check_name, round, status);
			CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
			break;
		}
	}
	CHECK(munmap(found, sizeof(*found)) == 0);
}

// Many forks one after another: each child's memory comes back.
static void many(void) {
	for(int i = 0; i < 200; i++) {
		pid_t pid = fork();
		if(pid == 0)
			_exit(i & 0x7f);
		if(pid < 0) {
			CHECK(pid > 0);
			return;
		}
		int status = check_wait(pid);
		if(!WIFEXITED(status) || WEXITSTATUS(status) != (i & 0x7f)) {
			CHECK(WIFEXITED(status) && WEXITSTATUS(status) == (i & 0x7f));
			return;
		}
	}
}

int main(void) {
	check_start("fork");
	RUN(copies);
	RUN(shares);
	RUN(shares_new_pages);
	RUN(shared_semaphores);
	RUN(shared_mutex);
	RUN(statuses);
	RUN(waits);
	RUN(descriptors);
	RUN(revoked_while_used);
	RUN(orphans);
	RUN(signal_at_start);
	RUN(many);
	return check_end();
}
