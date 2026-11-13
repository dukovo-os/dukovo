// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Scheduling and limits: nice levels as nice and setpriority set them,
// the sched_ calls, the CPUs a thread may run on, resource limits as
// setrlimit sets them and fork and exec keep them, the file descriptor,
// stack and file size limits, the limits of other processes as prlimit
// reads and sets them, and the file mode mask.

#include <fcntl.h>
#include <pthread.h>
#include <sched.h>
#include <sys/resource.h>
#include <sys/stat.h>

#include "check.h"

// The program, to run again (argv[0]).
static const char *self;

// Uses `depth` MiB of stack; returns `depth`.
__attribute__((noinline)) static int deep(int depth) {
	volatile char block[1 << 20];
	block[0] = 1;
	block[sizeof(block) - 1] = 1;
	int below = depth > 1 ? deep(depth - 1) : 0;
	return below + block[0] + block[sizeof(block) - 1] - 1;
}

// Run again by `limits` after exec: checks what exec kept, and that the
// stack is as large as RLIMIT_STACK says.
static int after_exec(void) {
	check_start("limits");
	struct rlimit limit;
	CHECK(getrlimit(RLIMIT_STACK, &limit) == 0 && limit.rlim_cur == 16 << 20);
	CHECK(getrlimit(RLIMIT_CORE, &limit) == 0 && limit.rlim_max == 0);
	CHECK(umask(0) == 027);
	// 12 MiB of stack: more than the 8 MiB a program gets by default.
	CHECK(deep(12) == 12);
	// The kernel still holds writes to the file size limit.
	CHECK(getrlimit(RLIMIT_FSIZE, &limit) == 0 && limit.rlim_cur == 1 << 20);
	signal(SIGXFSZ, SIG_IGN);
	int fd = open("/tmp/limits-exec", O_WRONLY | O_CREAT | O_TRUNC, 0600);
	CHECK(fd >= 0);
	CHECK(pwrite(fd, "0123456789", 10, (1 << 20) - 4) == 4);
	close(fd);
	CHECK(unlink("/tmp/limits-exec") == 0);
	return check_end();
}

// Runs `f` with an argument in a thread and returns what it returned.
static void *in_thread(void *(*f)(void *), void *argument) {
	pthread_t thread;
	void *result = NULL;
	CHECK(pthread_create(&thread, NULL, f, argument) == 0);
	CHECK(pthread_join(thread, &result) == 0);
	return result;
}

static void *thread_nice(void *argument) {
	(void)argument;
	int nice_was = getpriority(PRIO_PROCESS, 0);
	// Its own, as on Linux: the first thread's stays.
	setpriority(PRIO_PROCESS, 0, 10);
	return (void *)(intptr_t)nice_was;
}

static void priorities(void) {
	errno = 0;
	CHECK(getpriority(PRIO_PROCESS, 0) == 0 && errno == 0);
	CHECK(getpriority(PRIO_PROCESS, getpid()) == 0);
	// In a child in a process group of its own, so that this process
	// keeps its own.
	pid_t child = check_fork();
	if(child == 0) {
		CHECK(setpgid(0, 0) == 0);
		CHECK(nice(3) == 3);
		CHECK(getpriority(PRIO_PROCESS, 0) == 3);
		// A new thread starts with its maker's level.
		CHECK((intptr_t)in_thread(thread_nice, NULL) == 3);
		CHECK(getpriority(PRIO_PROCESS, 0) == 3);
		// More of the CPU needs RLIMIT_NICE, which is 0 as on Linux.
		CHECK_ERROR(setpriority(PRIO_PROCESS, 0, 1), EACCES);
		errno = 0;
		CHECK(nice(-1) == -1 && errno == EPERM);
		CHECK(setpriority(PRIO_PROCESS, 0, 100) == 0);
		CHECK(getpriority(PRIO_PROCESS, 0) == 19);
		// A process group's, and one of its children's.
		CHECK(setpriority(PRIO_PGRP, 0, 19) == 0 && getpriority(PRIO_PGRP, 0) == 19);
		pid_t grandchild = check_fork();
		if(grandchild == 0) {
			CHECK(getpriority(PRIO_PROCESS, 0) == 19);
			check_exit_child();
		}
		CHECK_CHILD(grandchild);
		check_exit_child();
	}
	CHECK_CHILD(child);
	CHECK(getpriority(PRIO_PROCESS, 0) == 0);
	// Every process is user 0's until there are users (Phase 13).
	errno = 0;
	CHECK(getpriority(PRIO_USER, 0) <= 0 && errno == 0);
	CHECK_ERROR(getpriority(PRIO_USER, 1000), ESRCH);
	CHECK_ERROR(getpriority(PRIO_PROCESS, 99999), ESRCH);
	CHECK_ERROR(getpriority(42, 0), EINVAL);
}

static void policies(void) {
	struct sched_param param = {0};
	CHECK(sched_getscheduler(0) == SCHED_OTHER);
	CHECK(sched_getparam(getpid(), &param) == 0 && param.sched_priority == 0);
	int policy = -1;
	CHECK(pthread_getschedparam(pthread_self(), &policy, &param) == 0);
	CHECK(policy == SCHED_OTHER && param.sched_priority == 0);
	CHECK(sched_get_priority_min(SCHED_FIFO) == 1 && sched_get_priority_max(SCHED_FIFO) == 99);
	CHECK(sched_get_priority_min(SCHED_RR) == 1 && sched_get_priority_max(SCHED_RR) == 99);
	CHECK(sched_get_priority_min(SCHED_OTHER) == 0 && sched_get_priority_max(SCHED_OTHER) == 0);
	CHECK_ERROR(sched_get_priority_max(42), EINVAL);
	CHECK(sched_yield() == 0);
	pid_t child = check_fork();
	if(child == 0) {
		CHECK(sched_setscheduler(0, SCHED_BATCH, &param) == 0);
		CHECK(sched_getscheduler(0) == SCHED_BATCH);
		CHECK(sched_setscheduler(getpid(), SCHED_OTHER, &param) == 0);
		// Real-time scheduling needs RLIMIT_RTPRIO (0, as on Linux).
		param.sched_priority = 10;
		CHECK_ERROR(sched_setscheduler(0, SCHED_FIFO, &param), EPERM);
		CHECK(pthread_setschedparam(pthread_self(), SCHED_RR, &param) == EPERM);
		CHECK_ERROR(sched_setscheduler(0, SCHED_OTHER, &param), EINVAL);
		param.sched_priority = 100;
		CHECK_ERROR(sched_setscheduler(0, SCHED_FIFO, &param), EINVAL);
		param.sched_priority = 0;
		CHECK_ERROR(sched_setscheduler(0, 4, &param), EINVAL);
		// Idle is the smallest share; leaving it needs RLIMIT_NICE.
		CHECK(sched_setscheduler(0, SCHED_IDLE, &param) == 0);
		CHECK(sched_getscheduler(0) == SCHED_IDLE);
		CHECK_ERROR(sched_setscheduler(0, SCHED_OTHER, &param), EPERM);
		check_exit_child();
	}
	CHECK_CHILD(child);
	CHECK(sched_getscheduler(0) == SCHED_OTHER);
	CHECK_ERROR(sched_getscheduler(99999), ESRCH);
}

static void *thread_cpus(void *argument) {
	(void)argument;
	cpu_set_t set;
	CPU_ZERO(&set);
	CHECK(pthread_getaffinity_np(pthread_self(), sizeof(set), &set) == 0);
	return (void *)(intptr_t)CPU_COUNT(&set);
}

// The CPUs a thread may run on: every online one at first; one that
// sched_setaffinity pins it to, which a new thread and a child start
// with; pthread_setaffinity_np's, and the errors.
static void affinity(void) {
	cpu_set_t all, one, got;
	CPU_ZERO(&all);
	CHECK(sched_getaffinity(0, sizeof(all), &all) == 0);
	int count = CPU_COUNT(&all);
	CHECK(count >= 1 && count == sysconf(_SC_NPROCESSORS_ONLN));
	int first = 0;
	while(first < CPU_SETSIZE - 1 && !CPU_ISSET(first, &all))
		first++;
	CPU_ZERO(&one);
	CPU_SET(first, &one);
	CHECK(sched_setaffinity(0, sizeof(one), &one) == 0);
	CPU_ZERO(&got);
	CHECK(sched_getaffinity(getpid(), sizeof(got), &got) == 0);
	CHECK(CPU_COUNT(&got) == 1 && CPU_ISSET(first, &got));
	CHECK((intptr_t)in_thread(thread_cpus, NULL) == 1);
	pid_t child = fork();
	if(child == 0) {
		cpu_set_t mine;
		CPU_ZERO(&mine);
		int pinned = sched_getaffinity(0, sizeof(mine), &mine) == 0 && CPU_COUNT(&mine) == 1
				&& CPU_ISSET(first, &mine);
		_exit(pinned ? 0 : 1);
	}
	int status;
	CHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
	CHECK(pthread_setaffinity_np(pthread_self(), sizeof(all), &all) == 0);
	CPU_ZERO(&got);
	CHECK(pthread_getaffinity_np(pthread_self(), sizeof(got), &got) == 0);
	CHECK(CPU_COUNT(&got) == count);
	cpu_set_t none;
	CPU_ZERO(&none);
	CHECK_ERROR(sched_setaffinity(0, sizeof(none), &none), EINVAL);
	CHECK_ERROR(sched_getaffinity(0, 1, &got), EINVAL);
	CHECK_ERROR(sched_getaffinity(99999, sizeof(got), &got), ESRCH);
	// A negative number names no process; sched_getscheduler calls it
	// invalid, as Linux does.
	CHECK_ERROR(sched_getaffinity(-1, sizeof(got), &got), ESRCH);
	CHECK_ERROR(sched_setaffinity(-1, sizeof(all), &all), ESRCH);
	CHECK_ERROR(sched_getscheduler(-1), EINVAL);
	CHECK(sched_setaffinity(0, sizeof(all), &all) == 0);
}

static void limits(void) {
	struct rlimit limit;
	CHECK(getrlimit(RLIMIT_NOFILE, &limit) == 0);
	CHECK(limit.rlim_cur == 1024 && limit.rlim_max == 4096);
	CHECK(sysconf(_SC_OPEN_MAX) == 1024);
	CHECK(getrlimit(RLIMIT_STACK, &limit) == 0 && limit.rlim_cur == 8 << 20);
	CHECK(limit.rlim_max == RLIM_INFINITY);
	CHECK(getrlimit(RLIMIT_CPU, &limit) == 0 && limit.rlim_cur == RLIM_INFINITY);
	CHECK(getrlimit(RLIMIT_NICE, &limit) == 0 && limit.rlim_max == 0);
	CHECK(getrlimit(RLIMIT_RTPRIO, &limit) == 0 && limit.rlim_max == 0);
	CHECK_ERROR(getrlimit(99, &limit), EINVAL);
	pid_t child = check_fork();
	if(child == 0) {
		// Descriptors stop at the soft limit.
		struct rlimit files = {16, 1024};
		CHECK(setrlimit(RLIMIT_NOFILE, &files) == 0);
		CHECK(sysconf(_SC_OPEN_MAX) == 16);
		int opened = 0;
		while(opened < 64 && open("/", O_RDONLY | O_DIRECTORY) >= 0)
			opened++;
		CHECK(errno == EMFILE && opened > 0 && opened < 16);
		CHECK_ERROR(dup2(0, 20), EBADF);
		CHECK_ERROR(fcntl(0, F_DUPFD, 20), EINVAL);
		files.rlim_cur = 1024;
		CHECK(setrlimit(RLIMIT_NOFILE, &files) == 0);
		CHECK(dup2(0, 20) == 20);
		// A hard limit only goes down; the soft one stays below it.
		files.rlim_max = 2048;
		CHECK_ERROR(setrlimit(RLIMIT_NOFILE, &files), EPERM);
		files.rlim_cur = 2000;
		files.rlim_max = 1024;
		CHECK_ERROR(setrlimit(RLIMIT_NOFILE, &files), EINVAL);
		struct rlimit core = {0, 0};
		CHECK(setrlimit(RLIMIT_CORE, &core) == 0);
		core.rlim_max = RLIM_INFINITY;
		CHECK_ERROR(setrlimit(RLIMIT_CORE, &core), EPERM);
		// A child has them, and exec keeps them, with a stack that size.
		struct rlimit stack = {16 << 20, RLIM_INFINITY};
		CHECK(setrlimit(RLIMIT_STACK, &stack) == 0);
		CHECK(umask(027) == 022);
		pid_t grandchild = check_fork();
		if(grandchild == 0) {
			CHECK(getrlimit(RLIMIT_CORE, &core) == 0 && core.rlim_max == 0);
			CHECK(umask(027) == 027);
			struct rlimit size = {1 << 20, RLIM_INFINITY};
			CHECK(setrlimit(RLIMIT_FSIZE, &size) == 0);
			execl(self, self, "after-exec", (char *)NULL);
			CHECK(!"exec");
			check_exit_child();
		}
		CHECK_CHILD(grandchild);
		check_exit_child();
	}
	CHECK_CHILD(child);
	CHECK(getrlimit(RLIMIT_CORE, &limit) == 0 && limit.rlim_max == RLIM_INFINITY);
}

// RLIMIT_FSIZE: a file grows no further than the soft limit. A write
// that crosses it is cut short, and one that starts at it is SIGXFSZ to
// the writer, or EFBIG where that signal is ignored or caught.
static void file_size(void) {
	pid_t child = check_fork();
	if(child == 0) {
		signal(SIGXFSZ, SIG_IGN);
		struct rlimit limit = {1024, RLIM_INFINITY};
		CHECK(setrlimit(RLIMIT_FSIZE, &limit) == 0);
		int fd = open("/tmp/limits-size", O_RDWR | O_CREAT | O_TRUNC, 0600);
		CHECK(fd >= 0);
		char block[2048];
		memset(block, 'f', sizeof(block));
		CHECK(write(fd, block, 1000) == 1000);
		CHECK(write(fd, block, 100) == 24);
		CHECK_ERROR(write(fd, block, 1), EFBIG);
		CHECK_ERROR(pwrite(fd, block, 1, 5000), EFBIG);
		CHECK(pwrite(fd, block, 10, 1020) == 4);
		// Nothing to write is no growth.
		CHECK(write(fd, block, 0) == 0);
		// A descriptor that appends starts at the end, which is the limit.
		int appending = open("/tmp/limits-size", O_WRONLY | O_APPEND);
		CHECK(appending >= 0);
		CHECK_ERROR(write(appending, block, 1), EFBIG);
		close(appending);
		// A file may be cut to any size up to the limit.
		CHECK_ERROR(ftruncate(fd, 5000), EFBIG);
		CHECK(ftruncate(fd, 10) == 0);
		CHECK(ftruncate(fd, 1024) == 0);
		// The limit is for files: a pipe takes more than it.
		int ends[2];
		CHECK(pipe(ends) == 0);
		CHECK(write(ends[1], block, sizeof(block)) == sizeof(block));
		close(ends[0]);
		close(ends[1]);
		// A writer that does not ignore SIGXFSZ is ended by it.
		pid_t grandchild = check_fork();
		if(grandchild == 0) {
			signal(SIGXFSZ, SIG_DFL);
			write(fd, block, 1);
			check_exit_child();
		}
		int status = check_wait(grandchild);
		CHECK(WIFSIGNALED(status) && WTERMSIG(status) == SIGXFSZ);
		close(fd);
		check_exit_child();
	}
	CHECK_CHILD(child);
	CHECK(unlink("/tmp/limits-size") == 0);
	struct rlimit limit;
	CHECK(getrlimit(RLIMIT_FSIZE, &limit) == 0 && limit.rlim_cur == RLIM_INFINITY);
}

// Whether `limit` is what getrlimit says of `resource` here.
static int is_ours(int resource, const struct rlimit *limit) {
	struct rlimit ours;
	return getrlimit(resource, &ours) == 0 && ours.rlim_cur == limit->rlim_cur
			&& ours.rlim_max == limit->rlim_max;
}

// prlimit: the limits of this process and of others, which it may read
// and set as far as it may signal them.
static void other_processes(void) {
	struct rlimit limit;
	CHECK(prlimit(0, RLIMIT_NOFILE, NULL, &limit) == 0 && is_ours(RLIMIT_NOFILE, &limit));
	CHECK(prlimit(getpid(), RLIMIT_NOFILE, NULL, &limit) == 0);
	CHECK(is_ours(RLIMIT_NOFILE, &limit));
	pid_t child = check_fork();
	if(child == 0) {
		struct rlimit more = {4096, RLIM_INFINITY};
		CHECK(prlimit(0, RLIMIT_CORE, &more, &limit) == 0);
		CHECK(limit.rlim_cur == 0 && limit.rlim_max == RLIM_INFINITY);
		CHECK(getrlimit(RLIMIT_CORE, &limit) == 0 && limit.rlim_cur == 4096);
		check_exit_child();
	}
	CHECK_CHILD(child);
	CHECK(getrlimit(RLIMIT_CORE, &limit) == 0 && limit.rlim_cur == 0);

	// No such process: none with a negative number, and none that has
	// been waited for.
	CHECK_ERROR(prlimit(-1, RLIMIT_NOFILE, NULL, &limit), ESRCH);
	pid_t gone = check_fork();
	if(gone == 0)
		_exit(0);
	CHECK_CHILD(gone);
	CHECK_ERROR(prlimit(gone, RLIMIT_NOFILE, NULL, &limit), ESRCH);

	// A child, held until its limit is set, which then applies to it.
	int go[2];
	CHECK(pipe(go) == 0);
	child = check_fork();
	if(child == 0) {
		close(go[1]);
		signal(SIGXFSZ, SIG_IGN);
		char byte;
		if(read(go[0], &byte, 1) != 1)
			_exit(2);
		char block[200];
		memset(block, 'g', sizeof(block));
		int fd = open("/tmp/limits-other", O_WRONLY | O_CREAT | O_TRUNC, 0600);
		ssize_t wrote = fd >= 0 ? write(fd, block, sizeof(block)) : -1;
		_exit(wrote < 0 ? 255 : (int)wrote);
	}
	close(go[0]);
	CHECK(prlimit(child, RLIMIT_STACK, NULL, &limit) == 0 && is_ours(RLIMIT_STACK, &limit));
	struct rlimit small = {100, RLIM_INFINITY};
	CHECK(prlimit(child, RLIMIT_FSIZE, &small, &limit) == 0);
	CHECK(limit.rlim_cur == RLIM_INFINITY && limit.rlim_max == RLIM_INFINITY);
	CHECK(prlimit(child, RLIMIT_FSIZE, NULL, &limit) == 0 && limit.rlim_cur == 100);
	CHECK(getrlimit(RLIMIT_FSIZE, &limit) == 0 && limit.rlim_cur == RLIM_INFINITY);
	// The descriptor limit is kept by the child's own C library, out of
	// reach of anything but the child: it can be read, not set.
	CHECK(prlimit(child, RLIMIT_NOFILE, NULL, &limit) == 0 && is_ours(RLIMIT_NOFILE, &limit));
	struct rlimit fewer = {512, 4096};
	CHECK_ERROR(prlimit(child, RLIMIT_NOFILE, &fewer, NULL), EPERM);
	CHECK(write(go[1], "g", 1) == 1);
	close(go[1]);
	int status = check_wait(child);
	CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 100);
	unlink("/tmp/limits-other");

	// Another session's processes are out of reach, as they are of kill:
	// Spark, process 1, is in one.
	pid_t stranger = check_fork();
	if(stranger == 0) {
		CHECK(setsid() == getpid());
		CHECK_ERROR(prlimit(1, RLIMIT_CORE, NULL, &limit), EPERM);
		struct rlimit none = {0, 0};
		CHECK_ERROR(prlimit(1, RLIMIT_CORE, &none, NULL), EPERM);
		check_exit_child();
	}
	CHECK_CHILD(stranger);
}

static void file_mask(void) {
	mode_t old = umask(077);
	CHECK(old == 022);
	CHECK(umask(old) == 077);
	CHECK(umask(01777) == 022);
	CHECK(umask(old) == 0777);
}

int main(int argc, char **argv) {
	self = argv[0];
	if(argc > 1 && !strcmp(argv[1], "after-exec"))
		return after_exec();
	check_start("limits");
	RUN(priorities);
	RUN(policies);
	RUN(affinity);
	RUN(limits);
	RUN(file_size);
	RUN(other_processes);
	RUN(file_mask);
	return check_end();
}
