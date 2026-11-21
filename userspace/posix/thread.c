// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// POSIX threads: creating and joining them, mutexes and condition
// variables, thread-local storage, detached threads, signals to a
// thread, thread names, descriptors two threads open at once, the
// memory of threads that have ended, which the process gets back, and a
// process that exits while another of its threads spins.

#include <fcntl.h>
#include <pthread.h>
#include <sched.h>
#include <stdio.h>
#include <stdatomic.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>

#include "check.h"

enum { THREADS = 8, ROUNDS = 2000 };

static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static long counter;
static atomic_long atomic_counter;
static _Thread_local int mine = 7;

static void *count(void *argument) {
	for(int i = 0; i < ROUNDS; i++) {
		pthread_mutex_lock(&lock);
		counter++;
		pthread_mutex_unlock(&lock);
		atomic_fetch_add(&atomic_counter, 1);
	}
	// Each thread has its own copy, which starts as the program's.
	int ok = mine == 7;
	mine = (int)(intptr_t)argument;
	sched_yield();
	ok = ok && mine == (int)(intptr_t)argument;
	return (void *)(intptr_t)(ok ? 100 + (intptr_t)argument : -1);
}

static void counts(void) {
	pthread_t threads[THREADS];
	for(intptr_t i = 0; i < THREADS; i++)
		CHECK(pthread_create(&threads[i], NULL, count, (void *)i) == 0);
	for(intptr_t i = 0; i < THREADS; i++) {
		void *result = NULL;
		CHECK(pthread_join(threads[i], &result) == 0);
		CHECK((intptr_t)result == 100 + i);
	}
	CHECK(counter == THREADS * ROUNDS);
	CHECK(atomic_load(&atomic_counter) == THREADS * ROUNDS);
	CHECK(mine == 7);
}

static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static int turn;

static void *ping(void *argument) {
	int me = (int)(intptr_t)argument;
	for(int i = 0; i < 1000; i++) {
		pthread_mutex_lock(&lock);
		while(turn % 2 != me)
			pthread_cond_wait(&changed, &lock);
		turn++;
		pthread_cond_broadcast(&changed);
		pthread_mutex_unlock(&lock);
	}
	return NULL;
}

static void conditions(void) {
	pthread_t threads[2];
	turn = 0;
	for(intptr_t i = 0; i < 2; i++)
		CHECK(pthread_create(&threads[i], NULL, ping, (void *)i) == 0);
	for(int i = 0; i < 2; i++)
		CHECK(pthread_join(threads[i], NULL) == 0);
	CHECK(turn == 2000);
	// A timed wait that times out.
	struct timespec until;
	clock_gettime(CLOCK_REALTIME, &until);
	until.tv_nsec += 20000000;
	if(until.tv_nsec >= 1000000000) {
		until.tv_sec++;
		until.tv_nsec -= 1000000000;
	}
	pthread_mutex_lock(&lock);
	CHECK(pthread_cond_timedwait(&changed, &lock, &until) == ETIMEDOUT);
	pthread_mutex_unlock(&lock);
	CHECK(pthread_mutex_trylock(&lock) == 0);
	CHECK(pthread_mutex_trylock(&lock) == EBUSY);
	pthread_mutex_unlock(&lock);
}

static atomic_int detached_ran;

static void *detached(void *argument) {
	(void)argument;
	atomic_store(&detached_ran, 1);
	pthread_exit(NULL);
}

static void *self(void *argument) {
	*(pthread_t *)argument = pthread_self();
	return NULL;
}

static void lives(void) {
	pthread_attr_t attr;
	pthread_attr_init(&attr);
	pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
	pthread_t thread;
	CHECK(pthread_create(&thread, &attr, detached, NULL) == 0);
	pthread_attr_destroy(&attr);
	for(int i = 0; i < 1000 && !atomic_load(&detached_ran); i++)
		check_sleep_ms(1);
	CHECK(atomic_load(&detached_ran));
	pthread_t seen;
	CHECK(pthread_create(&thread, NULL, self, &seen) == 0);
	CHECK(pthread_join(thread, NULL) == 0);
	CHECK(pthread_equal(seen, thread) && !pthread_equal(seen, pthread_self()));
	// A thread with a stack size of its own.
	pthread_attr_init(&attr);
	CHECK(pthread_attr_setstacksize(&attr, 1 << 20) == 0);
	CHECK(pthread_create(&thread, &attr, self, &seen) == 0);
	CHECK(pthread_join(thread, NULL) == 0);
	pthread_attr_destroy(&attr);
	// The first thread has the process's number, as on Linux.
	CHECK(gettid() == getpid());
}

static volatile pthread_t caught_on;
static atomic_int caught;

static void on_signal(int signal) {
	(void)signal;
	caught_on = pthread_self();
	atomic_store(&caught, 1);
}

static atomic_int stop;

static void *waits(void *argument) {
	(void)argument;
	while(!atomic_load(&stop))
		check_sleep_ms(1);
	return NULL;
}

// A signal sent to a thread runs its handler on that thread.
static void signals(void) {
	signal(SIGUSR1, on_signal);
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, waits, NULL) == 0);
	CHECK(pthread_kill(thread, SIGUSR1) == 0);
	for(int i = 0; i < 1000 && !atomic_load(&caught); i++)
		check_sleep_ms(1);
	CHECK(atomic_load(&caught) && pthread_equal(caught_on, thread));
	atomic_store(&stop, 1);
	CHECK(pthread_join(thread, NULL) == 0);
	// For the next one that waits (names' waiter, which must not end
	// while it is named).
	atomic_store(&stop, 0);
	signal(SIGUSR1, SIG_DFL);
}

// fork in a program with threads: the child has only the thread that
// called fork.
static void *forks(void *argument) {
	(void)argument;
	pid_t pid = fork();
	if(pid == 0)
		_exit(counter == THREADS * ROUNDS ? 0 : 1);
	int status;
	waitpid(pid, &status, 0);
	return (void *)(intptr_t)(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

static void forking(void) {
	pthread_t thread;
	fflush(stdout);
	CHECK(pthread_create(&thread, NULL, forks, NULL) == 0);
	void *result = NULL;
	CHECK(pthread_join(thread, &result) == 0 && result == (void *)1);
}

static void *named(void *argument) {
	char name[16];
	// A new thread has its process's name, until it names itself.
	int ok = pthread_getname_np(pthread_self(), name, sizeof(name)) == 0 && !strcmp(name, "thread");
	ok = ok && pthread_setname_np(pthread_self(), (const char *)argument) == 0;
	return (void *)(intptr_t)ok;
}

// pthread_setname_np and pthread_getname_np, with Linux's limit of 15
// bytes.
static void names(void) {
	char name[16];
	CHECK(pthread_getname_np(pthread_self(), name, sizeof(name)) == 0);
	CHECK(!strcmp(name, "thread"));
	CHECK(pthread_setname_np(pthread_self(), "main thread") == 0);
	CHECK(pthread_getname_np(pthread_self(), name, sizeof(name)) == 0);
	CHECK(!strcmp(name, "main thread"));
	CHECK(pthread_getname_np(pthread_self(), name, 11) == ERANGE);
	CHECK(pthread_getname_np(pthread_self(), name, 12) == 0 && !strcmp(name, "main thread"));
	CHECK(pthread_setname_np(pthread_self(), "fifteen bytes!!") == 0);
	CHECK(pthread_setname_np(pthread_self(), "sixteen bytes!!!") == ERANGE);
	CHECK(pthread_setname_np(pthread_self(), "\xff") == EINVAL);
	CHECK(pthread_setname_np(pthread_self(), "naïve") == 0);
	CHECK(pthread_getname_np(pthread_self(), name, sizeof(name)) == 0 && !strcmp(name, "naïve"));
	// A thread names itself, and this one names another.
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, named, "worker") == 0);
	void *result = NULL;
	CHECK(pthread_join(thread, &result) == 0 && result == (void *)1);
	CHECK(pthread_create(&thread, NULL, waits, NULL) == 0);
	CHECK(pthread_setname_np(thread, "waiter") == 0);
	CHECK(pthread_getname_np(thread, name, sizeof(name)) == 0 && !strcmp(name, "waiter"));
	atomic_store(&stop, 1);
	CHECK(pthread_join(thread, NULL) == 0);
	atomic_store(&stop, 0);
}

enum { FILES_MADE = 10000 };

static atomic_int opening;

// Opens a directory and closes it again until `opening` is 0, each time
// in the lowest free descriptor, as the other thread's opens are too.
// Returns how many times it did, or -1 if an open failed.
static void *opens(void *argument) {
	(void)argument;
	intptr_t times = 0;
	while(atomic_load(&opening)) {
		int fd = open("/tmp", O_RDONLY | O_DIRECTORY);
		if(fd < 0)
			return (void *)-1;
		close(fd);
		times++;
	}
	return (void *)times;
}

// Two threads open files at once, each in the lowest free descriptor: a
// file made with O_EXCL is made by one open, which succeeds. The C
// library once let both threads choose the same descriptor, and the open
// that lost it, its file already made, chose another and opened again,
// to fail with EEXIST on its own file.
static void descriptors(void) {
	atomic_store(&opening, 1);
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, opens, NULL) == 0);
	char path[64];
	for(int i = 0; i < FILES_MADE; i++) {
		snprintf(path, sizeof(path), "/tmp/thread-file-%d", i);
		int fd = open(path, O_WRONLY | O_CREAT | O_EXCL, 0644);
		CHECK(fd >= 0);
		if(fd >= 0)
			close(fd);
		CHECK(unlink(path) == 0);
		if(fd < 0)
			break;
	}
	atomic_store(&opening, 0);
	void *times = NULL;
	CHECK(pthread_join(thread, &times) == 0);
	CHECK((intptr_t)times > 0);
}

enum { ENDED = 300, STACK_USED = 16384 };

static atomic_int ended;

// Uses STACK_USED bytes of its stack, as a thread that does some work
// does, and counts itself among the ended.
static void *works(void *argument) {
	volatile char used[STACK_USED];
	memset((char *)used, 1, sizeof(used));
	atomic_fetch_add(&ended, 1);
	return argument;
}

// Waits until `count` threads in all have run works, for at most 10 s.
static void wait_ended(int count) {
	for(int i = 0; i < 10000 && atomic_load(&ended) < count; i++)
		check_sleep_ms(1);
	CHECK(atomic_load(&ended) >= count);
}

// Threads that end, joined, detached before they end and detached after
// they ended, ENDED of each, while the process's memory holds still: the
// C library gives back each thread's stack, its guard, its TLS and its
// key slots; kept, they would grow a server that starts a thread for each
// connection by some 25 KiB a connection for as long as it runs. A
// thread on a stack of its caller's (pthread_attr_setstack) leaves that
// stack to the caller, who can use it after the join.
static void given_back(void) {
	pthread_t thread;
	pthread_attr_t detach;
	pthread_attr_init(&detach);
	pthread_attr_setdetachstate(&detach, PTHREAD_CREATE_DETACHED);
	// The first round takes what the heap keeps for TCBs from then on.
	for(int i = 0; i < 20; i++) {
		CHECK(pthread_create(&thread, NULL, works, NULL) == 0);
		CHECK(pthread_join(thread, NULL) == 0);
	}
	atomic_store(&ended, 0);
	long before = check_anonymous_pages();
	CHECK(before > 0);
	for(int i = 0; i < ENDED; i++) {
		void *result = NULL;
		CHECK(pthread_create(&thread, NULL, works, &ended) == 0);
		CHECK(pthread_join(thread, &result) == 0 && result == &ended);
	}
	for(int i = 0; i < ENDED; i++) {
		CHECK(pthread_create(&thread, &detach, works, NULL) == 0);
		wait_ended(ENDED + i + 1);
	}
	for(int i = 0; i < ENDED; i++) {
		CHECK(pthread_create(&thread, NULL, works, NULL) == 0);
		wait_ended(2 * ENDED + i + 1);
		CHECK(pthread_detach(thread) == 0);
	}
	// The last detached threads' TCBs are freed by the next create.
	CHECK(pthread_create(&thread, NULL, works, NULL) == 0);
	CHECK(pthread_join(thread, NULL) == 0);
	long after = check_anonymous_pages();
	// 900 threads that kept what they used would hold over 5000 pages.
	if(after - before > 64)
		fprintf(stderr, "thread: %ld pages before %d threads ended, %ld after\n", before,
				3 * ENDED, after);
	CHECK(after - before <= 64);
	pthread_attr_destroy(&detach);

	static char own_stack[1 << 16] __attribute__((aligned(16)));
	pthread_attr_t attr;
	pthread_attr_init(&attr);
	CHECK(pthread_attr_setstack(&attr, own_stack, sizeof(own_stack)) == 0);
	CHECK(pthread_create(&thread, &attr, works, NULL) == 0);
	CHECK(pthread_join(thread, NULL) == 0);
	pthread_attr_destroy(&attr);
	memset(own_stack, 2, sizeof(own_stack));
	CHECK(own_stack[0] == 2 && own_stack[sizeof(own_stack) - 1] == 2);
}

// What the spinning thread and the test share, across the fork.
struct spinner {
	atomic_int spinning; // The thread has begun to spin.
	atomic_int release; // The test waited long enough and lets it go.
	atomic_int escaped; // The thread was let go: the exit never stopped it.
	cpu_set_t there; // The CPU it spins on.
};

static struct spinner *spinner;

// Spins on its own CPU, making no call to the kernel, until the process
// ends or the test lets it go.
static void *spins(void *argument) {
	(void)argument;
	if(CPU_COUNT(&spinner->there))
		CHECK(pthread_setaffinity_np(pthread_self(), sizeof(spinner->there), &spinner->there)
				== 0);
	atomic_store(&spinner->spinning, 1);
	while(!atomic_load_explicit(&spinner->release, memory_order_relaxed)) {
	}
	atomic_store(&spinner->escaped, 1);
	return NULL;
}

// A process ends while another of its threads spins on another CPU,
// making no call to the kernel: the kernel interrupts that CPU to stop
// the thread, which, running alone there, has no timer to bring it into
// the kernel. A child that has not ended in 10 s is let go, so a kernel
// that misses the thread fails the test and does not hold it.
static void spinning(void) {
	spinner = mmap(NULL, sizeof(*spinner), PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS,
			-1, 0);
	CHECK(spinner != MAP_FAILED);
	if(spinner == MAP_FAILED)
		return;
	// The exiting thread on the first CPU and the spinning one on the
	// second, when there are two.
	cpu_set_t all, here;
	CPU_ZERO(&all);
	CHECK(sched_getaffinity(0, sizeof(all), &all) == 0);
	int first = -1, second = -1;
	for(int cpu = 0; cpu < CPU_SETSIZE && second < 0; cpu++) {
		if(!CPU_ISSET(cpu, &all))
			continue;
		if(first < 0)
			first = cpu;
		else
			second = cpu;
	}
	CPU_ZERO(&here);
	CPU_ZERO(&spinner->there);
	if(second >= 0) {
		CPU_SET(first, &here);
		CPU_SET(second, &spinner->there);
	}
	pid_t pid = check_fork();
	if(pid == 0) {
		if(second >= 0)
			CHECK(sched_setaffinity(0, sizeof(here), &here) == 0);
		pthread_t thread;
		CHECK(pthread_create(&thread, NULL, spins, NULL) == 0);
		long long deadline = check_now_ns() + 5000000000LL;
		while(!atomic_load(&spinner->spinning) && check_now_ns() < deadline)
			check_sleep_ms(1);
		CHECK(atomic_load(&spinner->spinning));
		exit(check_failures > 125 ? 125 : check_failures);
	}
	long long deadline = check_now_ns() + 10000000000LL;
	int status;
	pid_t got;
	while((got = waitpid(pid, &status, WNOHANG)) == 0 && check_now_ns() < deadline)
		check_sleep_ms(1);
	if(got != pid) {
		atomic_store(&spinner->release, 1);
		status = check_wait(pid);
	}
	CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
	CHECK(!atomic_load(&spinner->escaped));
	munmap(spinner, sizeof(*spinner));
}

int main(void) {
	check_start("thread");
	RUN(counts);
	RUN(conditions);
	RUN(lives);
	RUN(signals);
	RUN(forking);
	RUN(names);
	RUN(descriptors);
	RUN(given_back);
	RUN(spinning);
	return check_end();
}
