// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a static Linux program relies on, asked of the system through the
// calls themselves: threads from the C library and from a raw clone, the
// process calls, the working directory, the memory calls, directories and
// file status, signals that interrupt a blocked read, interval timers,
// futexes with absolute timeouts, bit sets, requeues and wake-ops, robust
// futex lists, what an unprivileged program may not do,
// the descriptor and identity rules, and a pseudo-terminal's settings.
// Where a C library would hide a call's own rules (the raw clone, brk,
// getcwd, getdents64, statx, futex, the interval timers), the program
// makes the call itself, so that it is the system's answer that is checked.
//
// Each case prints one line, `linux-core: <case> ok`, or says what failed
// and with which errno, or `linux-core: <case> skip (<why>)` when the case
// has nothing to run on. The program ends with `linux-core: all <N> passed`
// and exits with 0, or exits with 1 after saying how many cases failed.
//
// It passes on a Linux machine, which is how its expectations are known to
// be Linux's: ob builds it with glibc and with musl and the `linux-programs`
// boot test runs both on Dukovo, where it runs under the Linux
// personality (ADR 0610). Where Dukovo is known to answer otherwise for
// now, the case says so and tests what each system promises.
//
//   linux-core [--native PROGRAM] [--native-pwd PROGRAM] [--started-in DIR]
//              [CASE...]
//
// With case names it runs only those. `--native` names a program of the
// Dukovo system that exits with 0 when run without arguments, and
// `--native-pwd` one that prints its working directory when given `-P`
// (the cases exec_native and native_cwd have nothing to run without them);
// `--started-in` names the directory the shell that started this program
// was in (the case started_in). `--exit N` and `--cwd` are what the program
// runs itself with: exit with N at once, and print the working directory.

#include <errno.h>
#include <dirent.h>
#include <fcntl.h>
#include <ftw.h>
#include <limits.h>
#include <pthread.h>
#include <sched.h>
#include <setjmp.h>
#include <signal.h>
#include <stddef.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/ioctl.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <sys/types.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

// The kernel's own numbers for the futex operations, which the C libraries'
// headers do not all carry (musl's has no linux/futex.h at all).
#define FUTEX_WAIT 0
#define FUTEX_WAKE 1
#define FUTEX_REQUEUE 3
#define FUTEX_CMP_REQUEUE 4
#define FUTEX_WAKE_OP 5
#define FUTEX_WAIT_BITSET 9
#define FUTEX_WAKE_BITSET 10
#define FUTEX_PRIVATE_FLAG 128
#define FUTEX_CLOCK_REALTIME 256
#define FUTEX_BITSET_MATCH_ANY 0xffffffffU
#define FUTEX_OP_SET 0
#define FUTEX_OP_ADD 1
#define FUTEX_OP_OR 2
#define FUTEX_OP_OPARG_SHIFT 8
#define FUTEX_OP_CMP_EQ 0
#define FUTEX_OP_CMP_GT 4
#define FUTEX_OP(op, oparg, cmp, cmparg) \
	((((unsigned)(op) & 0xf) << 28) | (((unsigned)(cmp) & 0xf) << 24) | \
			(((unsigned)(oparg) & 0xfff) << 12) | ((unsigned)(cmparg) & 0xfff))
#define FUTEX_WAITERS 0x80000000U
#define FUTEX_OWNER_DIED 0x40000000U

// The kernel's statx record, as linux/stat.h has it: the C libraries' own
// headers have it only in newer releases, and the call's layout is what the
// case checks.
struct linux_statx_timestamp {
	int64_t tv_sec;
	uint32_t tv_nsec;
	int32_t reserved;
};

struct linux_statx {
	uint32_t stx_mask;
	uint32_t stx_blksize;
	uint64_t stx_attributes;
	uint32_t stx_nlink;
	uint32_t stx_uid;
	uint32_t stx_gid;
	uint16_t stx_mode;
	uint16_t spare0;
	uint64_t stx_ino;
	uint64_t stx_size;
	uint64_t stx_blocks;
	uint64_t stx_attributes_mask;
	struct linux_statx_timestamp stx_atime, stx_btime, stx_ctime, stx_mtime;
	uint32_t stx_rdev_major, stx_rdev_minor, stx_dev_major, stx_dev_minor;
	uint64_t stx_mnt_id;
	uint32_t stx_dio_mem_align, stx_dio_offset_align;
	uint64_t spare3[12];
};

_Static_assert(sizeof(struct linux_statx) == 256, "the statx record is 256 bytes");

#define LINUX_STATX_BASIC_STATS 0x7ffU

#ifndef SYS_io_uring_setup
#define SYS_io_uring_setup 425
#endif
#ifndef ARCH_SET_FS
#define ARCH_SET_FS 0x1002
#define ARCH_GET_FS 0x1003
#endif

#define SOURCE "core.c"
#define PAGE 4096UL

// The longest a case may take before the watchdog ends the program: a
// case that waits for something that never comes must fail by name, not
// hold the boot test for its whole time.
#define CASE_LIMIT_SECONDS 30

// The name of the case that runs, which every line it prints starts with,
// how many checks of it failed, and whether it had nothing to run on.
static const char *volatile case_name = "";
static int case_failures;
static int case_skipped;

// Whether the system says it is Dukovo, in its release and version: a few
// answers are known to differ there for now.
static int on_dukovo;

// The options of the program (see the top of this file).
static const char *native_program;
static const char *native_pwd_program;
static const char *started_in;

// This program's path and the directory it started in, for the cases that
// run it again or come back to where they began.
static char self_path[2 * PATH_MAX];
static char start_dir[PATH_MAX];

// The directory this run's files are in.
static char scratch[256];

// When the running case began, in nanoseconds on the monotonic clock; 0
// between cases.
static atomic_llong case_started;

// Says that a check failed, with its line and the errno it saw, and counts it.
static void check_failed(int line, const char *what, int error) {
	printf("linux-core: %s: FAILED: " SOURCE ":%d: %s (errno %d: %s)\n", case_name, line, what,
			error, strerror(error));
	fflush(stdout);
	case_failures++;
}

// Checks that `condition` holds; if not, says so, with errno as the
// condition left it (the error of the call in it, if one failed).
#define CHECK(condition) \
	do { \
		int check_ok_ = !!(condition); \
		int check_errno_ = errno; \
		if(!check_ok_) \
			check_failed(__LINE__, #condition, check_errno_); \
	} while(0)

// Checks that `call` failed with -1 and errno `error`.
#define CHECK_ERROR(call, error) \
	do { \
		errno = 0; \
		long check_result_ = (long)(call); \
		int check_errno_ = errno; \
		if(check_result_ != -1 || check_errno_ != (error)) \
			check_failed(__LINE__, #call " fails with " #error, check_errno_); \
	} while(0)

// Checks that child `pid` ended by exiting with `code`.
#define CHECK_EXITED(pid, code) \
	do { \
		int check_status_ = wait_raw(pid); \
		CHECK(WIFEXITED(check_status_) && WEXITSTATUS(check_status_) == (code)); \
	} while(0)

// Says something about the running case that is no failure.
static void note(const char *text) {
	printf("linux-core: %s: %s\n", case_name, text);
	fflush(stdout);
}

// Fails the running case, saying `what` is wrong.
#define FAIL(what) check_failed(__LINE__, what, errno)

// Says that the running case has nothing to run on.
static void skip(const char *why) {
	printf("linux-core: %s skip (%s)\n", case_name, why);
	fflush(stdout);
	case_skipped = 1;
}

// Nanoseconds on `clock`.
static long long now_ns(clockid_t clock) {
	struct timespec now;
	clock_gettime(clock, &now);
	return (long long)now.tv_sec * 1000000000LL + now.tv_nsec;
}

// `ns` nanoseconds as the absolute time a futex call takes, on the clock they
// were read from.
static struct timespec timespec_of(long long ns) {
	struct timespec at = {(time_t)(ns / 1000000000LL), (long)(ns % 1000000000LL)};
	return at;
}

// Sleeps for `ms` milliseconds, however often a signal cuts it short.
static void sleep_ms(long ms) {
	struct timespec left = {ms / 1000, (ms % 1000) * 1000000L};
	while(nanosleep(&left, &left) < 0 && errno == EINTR) {
	}
}

// Waits for child `pid` with the raw wait4 call, which also fills in the
// child's resource usage, and returns its wait status, or -1 if waiting
// fails.
static int wait_raw(pid_t pid) {
	int status = 0;
	struct rusage usage;
	long got;
	do {
		got = syscall(SYS_wait4, (long)pid, &status, 0L, &usage);
	} while(got < 0 && errno == EINTR);
	if(got != pid) {
		check_failed(__LINE__, "wait4 returned the child", errno);
		return -1;
	}
	return status;
}

// The path of `name` in this run's directory, in `buffer`.
static const char *in_scratch(char *buffer, size_t size, const char *name) {
	snprintf(buffer, size, "%s/%s", scratch, name);
	return buffer;
}

// Makes the directory `name` in this run's directory and returns its path
// in `buffer`.
static const char *make_directory(char *buffer, size_t size, const char *name) {
	in_scratch(buffer, size, name);
	CHECK(mkdir(buffer, 0755) == 0);
	return buffer;
}

// Makes the file `path` with `length` bytes of `fill` and mode `mode`.
static void make_file(const char *path, size_t length, char fill, mode_t mode) {
	int fd = open(path, O_WRONLY | O_CREAT | O_EXCL, mode);
	CHECK(fd >= 0);
	if(fd < 0)
		return;
	CHECK(fchmod(fd, mode) == 0);
	char chunk[512];
	memset(chunk, fill, sizeof(chunk));
	for(size_t done = 0; done < length;) {
		size_t part = length - done < sizeof(chunk) ? length - done : sizeof(chunk);
		ssize_t wrote = write(fd, chunk, part);
		CHECK(wrote == (ssize_t)part);
		if(wrote <= 0)
			break;
		done += (size_t)wrote;
	}
	CHECK(close(fd) == 0);
}

// The signal action that a SIGSEGV takes to get back to `fault_jump`.
static sigjmp_buf fault_jump;

// Goes back to the probe in `faults` that expected the SIGSEGV.
static void on_fault(int signal_number) {
	(void)signal_number;
	siglongjmp(fault_jump, 1);
}

// Whether reading the byte at `address` raises SIGSEGV. The signal mask is
// saved and restored by sigsetjmp, so that the next fault is delivered
// too.
static int faults(volatile char *address) {
	struct sigaction act, old;
	memset(&act, 0, sizeof(act));
	act.sa_handler = on_fault;
	sigemptyset(&act.sa_mask);
	sigaction(SIGSEGV, &act, &old);
	int faulted = sigsetjmp(fault_jump, 1) != 0;
	if(!faulted)
		(void)*address;
	sigaction(SIGSEGV, &old, NULL);
	return faulted;
}

// Maps `pages` pages of anonymous private memory, or fails the case.
static char *map_pages(size_t pages) {
	void *at = mmap(NULL, pages * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(at != MAP_FAILED);
	return at == MAP_FAILED ? NULL : at;
}

// Fills page `page` of `at` with `value`.
static void fill_page(char *at, size_t page, char value) {
	memset(at + page * PAGE, value, PAGE);
}

// Whether every byte of page `page` of `at` is `value`.
static int page_is(const char *at, size_t page, char value) {
	for(size_t i = 0; i < PAGE; i++)
		if(at[page * PAGE + i] != value)
			return 0;
	return 1;
}

// Threads. Each thread the C library makes has words of its own in thread
// storage, a thread number of its own, and ends by clearing a word the
// joiner waits on.

static __thread int thread_word;

struct worker {
	int number;
	int word_seen;
	pid_t tid;
};

static atomic_int workers_done;

// What each thread of `libc_threads` does: it keeps a number of its own in
// its thread storage across a yield, and reports it and its thread number.
static void *worker_body(void *argument) {
	struct worker *worker = argument;
	thread_word = worker->number;
	sched_yield();
	worker->word_seen = thread_word;
	worker->tid = (pid_t)syscall(SYS_gettid);
	atomic_fetch_add(&workers_done, 1);
	return (void *)(intptr_t)(worker->number * 2);
}

// Threads made with the C library's pthread_create, which asks for a
// clone with CLONE_VM, FS, FILES, SIGHAND, THREAD, SYSVSEM, SETTLS,
// PARENT_SETTID and CHILD_CLEARTID: four at once, each with its own thread
// storage, and a hundred one after another, each joined by the word its end
// clears.
static void libc_threads(void) {
	enum { COUNT = 4 };
	struct worker workers[COUNT];
	pthread_t threads[COUNT];
	pid_t main_tid = (pid_t)syscall(SYS_gettid);
	thread_word = -1;
	atomic_store(&workers_done, 0);
	for(int i = 0; i < COUNT; i++) {
		workers[i] = (struct worker){.number = i + 1};
		CHECK(pthread_create(&threads[i], NULL, worker_body, &workers[i]) == 0);
	}
	for(int i = 0; i < COUNT; i++) {
		void *result = NULL;
		CHECK(pthread_join(threads[i], &result) == 0);
		CHECK((intptr_t)result == (i + 1) * 2);
		CHECK(workers[i].word_seen == i + 1);
		CHECK(workers[i].tid != main_tid && workers[i].tid != 0);
		for(int j = 0; j < i; j++)
			CHECK(workers[i].tid != workers[j].tid);
	}
	CHECK(atomic_load(&workers_done) == COUNT);
	CHECK(thread_word == -1);
	for(int round = 0; round < 100; round++) {
		struct worker worker = {.number = round};
		pthread_t thread;
		void *result = NULL;
		if(pthread_create(&thread, NULL, worker_body, &worker) != 0 ||
				pthread_join(thread, &result) != 0) {
			FAIL("a thread is made and joined");
			return;
		}
		if((intptr_t)result != round * 2) {
			FAIL("a thread's result reaches its joiner");
			return;
		}
	}
}

// The flags the C library gives pthread_create's clone.
#define THREAD_FLAGS \
	(CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM | \
			CLONE_SETTLS | CLONE_PARENT_SETTID | CLONE_CHILD_CLEARTID)

// The first word of the thread storage the raw thread is given, which it
// reads back through %fs.
#define TLS_MAGIC 0x544c53544c53ULL
#define THREAD_MAGIC 0x5a5a5a5aL

// The raw clone call, then the new thread's whole life in assembly: it
// finds the address of `words` on its own stack, writes THREAD_MAGIC to
// the first word and what %fs:0 holds to the second, and ends with the exit
// call, which ends the thread alone and clears the word that CHILD_CLEARTID
// names. The new thread cannot return into the C code that made it: it has
// no frame of that code on its stack. Returns what the call returns: the
// thread's number in the caller, or the negated errno.
static long clone_thread(unsigned long flags, long *stack_top, int *parent_tid, int *child_tid,
		void *tls) {
	register long r10 __asm__("r10") = (long)child_tid;
	register long r8 __asm__("r8") = (long)tls;
	long result;
	__asm__ __volatile__(
			"syscall\n\t"
			"test %%rax, %%rax\n\t"
			"jnz 1f\n\t"
			"pop %%r9\n\t"
			"movq %[magic], %%rax\n\t"
			"movq %%rax, (%%r9)\n\t"
			"movq %%fs:0, %%rax\n\t"
			"movq %%rax, 8(%%r9)\n\t"
			"movl %[exit], %%eax\n\t"
			"xorl %%edi, %%edi\n\t"
			"syscall\n\t"
			"ud2\n\t"
			"1:\n\t"
			: "=a"(result)
			: "0"((long)SYS_clone), "D"(flags), "S"(stack_top), "d"(parent_tid), "r"(r10),
			  "r"(r8), [magic] "i"(THREAD_MAGIC), [exit] "i"(SYS_exit)
			: "rcx", "r11", "r9", "memory");
	return result;
}

// A thread made by a raw clone on a stack of the case's own: its number
// is written to the parent's word before either side runs on, it runs with
// the thread storage it was given, and when it ends with the exit call the
// system clears the word it named and wakes a waiter on it. The kernel
// wakes that word as a shared futex, so the wait is not a private one.
static void raw_thread(void) {
	size_t stack_size = 16 * PAGE;
	char *stack = mmap(NULL, stack_size, PROT_READ | PROT_WRITE,
			MAP_PRIVATE | MAP_ANONYMOUS | MAP_STACK, -1, 0);
	CHECK(stack != MAP_FAILED);
	if(stack == MAP_FAILED)
		return;
	static volatile long words[2];
	static volatile int parent_tid, cleared;
	static long tls_block[8];
	words[0] = words[1] = 0;
	parent_tid = 0;
	cleared = 1;
	tls_block[0] = (long)TLS_MAGIC;
	long *stack_top = (long *)(stack + stack_size);
	*--stack_top = (long)words;
	long tid = clone_thread(THREAD_FLAGS, stack_top, (int *)&parent_tid, (int *)&cleared,
			tls_block);
	CHECK(tid > 0);
	if(tid > 0) {
		CHECK(parent_tid == tid);
		CHECK(tid != getpid());
		long long deadline = now_ns(CLOCK_MONOTONIC) + 10000000000LL;
		struct timespec wait = {1, 0};
		int value;
		while((value = cleared) != 0 && now_ns(CLOCK_MONOTONIC) < deadline)
			syscall(SYS_futex, (int *)&cleared, FUTEX_WAIT, value, &wait, NULL, 0);
		CHECK(cleared == 0);
		CHECK(words[0] == THREAD_MAGIC);
		CHECK(words[1] == (long)TLS_MAGIC);
	}
	if(cleared == 0)
		CHECK(munmap(stack, stack_size) == 0);
}

// The two ways of making threads, the C library's and a raw clone.
static void threads(void) {
	libc_threads();
	raw_thread();
}

// gettid: the first thread's number is the process's, a thread of the
// process has another, and the thread number names a thread to tgkill for
// as long as the thread lives.

struct tid_report {
	pid_t tid;
	pid_t pid;
	atomic_int release;
};

// Tells `gettid_case` its thread number and process number, then waits to
// be released.
static void *report_tid(void *argument) {
	struct tid_report *report = argument;
	report->tid = (pid_t)syscall(SYS_gettid);
	report->pid = getpid();
	while(!atomic_load(&report->release))
		sleep_ms(1);
	return NULL;
}

// The first thread's number is the process's, and a thread of it has another
// that tgkill takes.
static void gettid_case(void) {
	pid_t pid = getpid();
	CHECK(syscall(SYS_gettid) == pid);
	static struct tid_report report;
	atomic_store(&report.release, 0);
	report.tid = 0;
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, report_tid, &report) == 0);
	for(int i = 0; i < 2000 && !report.tid; i++)
		sleep_ms(1);
	CHECK(report.tid != 0 && report.tid != pid);
	CHECK(report.pid == pid);
	CHECK(syscall(SYS_tgkill, (long)pid, (long)report.tid, 0L) == 0);
	CHECK(syscall(SYS_gettid) == pid);
	atomic_store(&report.release, 1);
	CHECK(pthread_join(thread, NULL) == 0);
	CHECK_ERROR(syscall(SYS_tgkill, (long)pid, (long)report.tid, 0L), ESRCH);
}

// fork written as the raw clone call: with CLONE_CHILD_SETTID the child's
// thread number is written in the child's memory and the parent's copy of
// the word stays as it was; the child is the first thread of a new process,
// so its number is its process's. The raw wait4 call gives the child's
// exit code, and no child is left.
static void fork_raw(void) {
	volatile pid_t tid = 0;
	fflush(stdout);
	long pid = syscall(SYS_clone, (long)(SIGCHLD | CLONE_CHILD_SETTID), 0L, 0L, (long)&tid, 0L);
	if(pid == 0)
		_exit(tid != 0 && tid == syscall(SYS_gettid) && tid == getpid() ? 0 : 11);
	CHECK(pid > 0);
	CHECK(tid == 0);
	CHECK_EXITED(pid, 0);
	pid = syscall(SYS_clone, (long)SIGCHLD, 0L, 0L, 0L, 0L);
	if(pid == 0)
		_exit(42);
	CHECK(pid > 0);
	CHECK_EXITED(pid, 42);
	int status;
	CHECK_ERROR(syscall(SYS_wait4, -1L, &status, (long)WNOHANG, 0L), ECHILD);
}

// A vfork whose child waits `delay` and then runs `path`. Nothing but the
// calls the child makes may happen between vfork and the exec: the parent's
// memory may be the child's.
static __attribute__((noinline)) pid_t vfork_exec(const char *path, char *const argv[],
		const struct timespec *delay) {
	pid_t pid = vfork();
	if(pid == 0) {
		(void)syscall(SYS_nanosleep, delay, NULL);
		execv(path, argv);
		_exit(127);
	}
	return pid;
}

// A vfork whose child ends with `code` at once.
static __attribute__((noinline)) pid_t vfork_exit(int code) {
	pid_t pid = vfork();
	if(pid == 0)
		_exit(code);
	return pid;
}

// vfork: the parent does not run again until the child has run another
// program (here this one, which ends with 7 at once) or ended, whatever the
// child does first, and the status is the child's; a child that ends
// without an exec holds its parent just the same.
static void vfork_case(void) {
	char code[] = "7";
	char *const argv[] = {self_path, "--exit", code, NULL};
	struct timespec delay = {0, 200 * 1000000L};
	fflush(stdout);
	long long before = now_ns(CLOCK_MONOTONIC);
	pid_t pid = vfork_exec(self_path, argv, &delay);
	long long held = now_ns(CLOCK_MONOTONIC) - before;
	CHECK(pid > 0);
	CHECK(held >= 180 * 1000000LL);
	CHECK_EXITED(pid, 7);
	pid = vfork_exit(3);
	CHECK(pid > 0);
	CHECK_EXITED(pid, 3);
}

// What a program run as `--cwd` printed, which is its working directory,
// read from `fd` to its end into `buffer`.
static void read_all(int fd, char *buffer, size_t size) {
	size_t got = 0;
	for(;;) {
		ssize_t n = read(fd, buffer + got, size - 1 - got);
		if(n < 0 && errno == EINTR)
			continue;
		if(n <= 0)
			break;
		got += (size_t)n;
	}
	buffer[got] = 0;
}

// Goes to `directory` and back, and puts in `real` where getcwd places it,
// which is the path a program started there is to find.
static void real_path_of(const char *directory, char *real, size_t size) {
	real[0] = 0;
	CHECK(chdir(directory) == 0);
	CHECK(getcwd(real, size) == real);
	CHECK(chdir(start_dir) == 0);
}

// Runs `program` with `argv` in a child that first goes to `directory`, and
// puts what the child writes to its standard output in `printed`. Returns
// the child's exit status, or -1 if it did not exit.
static int run_in(const char *directory, const char *program, char *const argv[], char *printed,
		size_t size) {
	int fds[2];
	CHECK(pipe(fds) == 0);
	fflush(stdout);
	pid_t pid = fork();
	if(pid == 0) {
		if(chdir(directory) != 0 || dup2(fds[1], STDOUT_FILENO) < 0)
			_exit(126);
		close(fds[0]);
		close(fds[1]);
		execv(program, argv);
		_exit(127);
	}
	CHECK(pid > 0);
	close(fds[1]);
	read_all(fds[0], printed, size);
	close(fds[0]);
	int status = wait_raw(pid);
	return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

// execve keeps the working directory: a program run from a directory the
// caller went to finds itself there.
static void execve_cwd(void) {
	char directory[512], real[PATH_MAX], printed[PATH_MAX + 2], expected[PATH_MAX + 2];
	make_directory(directory, sizeof(directory), "exec-cwd");
	real_path_of(directory, real, sizeof(real));
	char *const argv[] = {self_path, "--cwd", NULL};
	CHECK(run_in(directory, self_path, argv, printed, sizeof(printed)) == 0);
	snprintf(expected, sizeof(expected), "%s\n", real);
	CHECK(!strcmp(printed, expected));
}

// A program of the Dukovo system, run from here: it starts, ends with 0,
// and a program that is not there is not run.
static void exec_native(void) {
	if(!native_program) {
		skip("no --native program");
		return;
	}
	char *const argv[] = {(char *)native_program, NULL};
	fflush(stdout);
	pid_t pid = fork();
	if(pid == 0) {
		execv(native_program, argv);
		_exit(127);
	}
	CHECK(pid > 0);
	CHECK_EXITED(pid, 0);
	pid = fork();
	if(pid == 0) {
		execv("/linux-core-no-such-program", argv);
		_exit(errno == ENOENT ? 0 : 1);
	}
	CHECK(pid > 0);
	CHECK_EXITED(pid, 0);
}

// The working directory crosses to a program of the Dukovo system: one that
// prints its own, run from a directory this program went to, prints that
// one.
static void native_cwd(void) {
	if(!native_pwd_program) {
		skip("no --native-pwd program");
		return;
	}
	char directory[512], real[PATH_MAX], printed[PATH_MAX + 2], expected[PATH_MAX + 2];
	make_directory(directory, sizeof(directory), "native-cwd");
	real_path_of(directory, real, sizeof(real));
	char *const argv[] = {(char *)native_pwd_program, "-P", NULL};
	CHECK(run_in(directory, native_pwd_program, argv, printed, sizeof(printed)) == 0);
	snprintf(expected, sizeof(expected), "%s\n", real);
	CHECK(!strcmp(printed, expected));
}

// A program started by another system keeps the directory the program that
// started it was in: this one finds itself in `--started-in`.
static void started_in_case(void) {
	if(!started_in) {
		skip("no --started-in directory");
		return;
	}
	CHECK(!strcmp(start_dir, started_in));
}

// The calls of the C library's own thread storage: arch_prctl sets and
// reads the base of %fs. Between the two settings only code that makes no
// call into the C library runs, which reads its thread storage through %fs
// itself.

// The arch_prctl call, made without the C library.
static inline long raw_arch_prctl(long code, unsigned long argument) {
	long result;
	__asm__ __volatile__("syscall"
			: "=a"(result)
			: "0"((long)SYS_arch_prctl), "D"(code), "S"(argument)
			: "rcx", "r11", "memory");
	return result;
}

// The first word at the base of %fs.
static inline unsigned long fs_word(void) {
	unsigned long word;
	__asm__ __volatile__("movq %%fs:0, %0" : "=r"(word));
	return word;
}

// Sets the base of %fs to a buffer whose first word is known, reads the word
// through %fs, asks for the base back, and puts the C library's own back.
// Returns 0 if each step gave what it should, else the number of the step.
static __attribute__((noinline)) int switch_fs(unsigned long *buffer) {
	unsigned long original = 0, current = 0;
	if(raw_arch_prctl(ARCH_GET_FS, (unsigned long)&original) != 0)
		return 1;
	if(raw_arch_prctl(ARCH_SET_FS, (unsigned long)buffer) != 0)
		return 2;
	unsigned long word = fs_word();
	long got = raw_arch_prctl(ARCH_GET_FS, (unsigned long)&current);
	raw_arch_prctl(ARCH_SET_FS, original);
	if(word != buffer[0])
		return 3;
	if(got != 0 || current != (unsigned long)buffer)
		return 4;
	return 0;
}

// arch_prctl sets and reads the base of %fs, and refuses a code it does not
// know.
static void arch_prctl_case(void) {
	static unsigned long buffer[8];
	unsigned long before = 0, after = 0;
	buffer[0] = 0x464c4f4f4f4cUL;
	CHECK(raw_arch_prctl(ARCH_GET_FS, (unsigned long)&before) == 0);
	CHECK(switch_fs(buffer) == 0);
	CHECK(raw_arch_prctl(ARCH_GET_FS, (unsigned long)&after) == 0);
	CHECK(before == after && before != 0);
	// A code the call does not know is refused, as a raw call says it:
	// with the negated errno.
	CHECK(raw_arch_prctl(0x7777, 0) == -EINVAL);
}

// brk, as the raw call makes it: no argument gives the break; the break
// grows by pages that can all be written, shrinks back, and gives up
// the pages above it; an address below where the break began changes
// nothing and gives the break as it is. Nothing allocates between the
// calls, since the C library's allocator keeps its own idea of the break.
static unsigned long raw_brk(unsigned long address) {
	return (unsigned long)syscall(SYS_brk, address);
}

// The program break grows and shrinks as the comment above says: the break
// is the address asked for, the pages between can be written, the pages
// above it are gone, and an address below where it began leaves it as it was.
static void brk_case(void) {
	unsigned long start = raw_brk(0);
	CHECK(start != 0);
	unsigned long grown = raw_brk(start + 3 * PAGE);
	CHECK(grown == start + 3 * PAGE);
	if(grown == start + 3 * PAGE) {
		volatile char *bytes = (volatile char *)start;
		for(unsigned long i = 0; i < 3 * PAGE; i++)
			bytes[i] = (char)(i * 13 + 1);
		unsigned long wrong = 0;
		for(unsigned long i = 0; i < 3 * PAGE; i++)
			wrong += bytes[i] != (char)(i * 13 + 1);
		CHECK(wrong == 0);
	}
	CHECK(raw_brk(0) == grown);
	CHECK(raw_brk(start) == start);
	CHECK(raw_brk(0) == start);
	// The first page boundary at or above the break is not the process's
	// any more.
	CHECK(faults((volatile char *)((start + PAGE - 1) & ~(PAGE - 1))));
	CHECK(raw_brk(PAGE) == start);
	CHECK(raw_brk(0) == start);
}

// madvise(MADV_DONTNEED) on private anonymous memory gives the pages back
// and keeps the mapping: the next read finds zeros, and the pages are
// writable again. An address that is not a page's and advice that does not
// exist are refused.
static void madvise_dontneed(void) {
	char *at = map_pages(2);
	if(!at)
		return;
	fill_page(at, 0, (char)0xaa);
	fill_page(at, 1, (char)0xaa);
	CHECK(page_is(at, 0, (char)0xaa) && page_is(at, 1, (char)0xaa));
	CHECK(madvise(at, 2 * PAGE, MADV_DONTNEED) == 0);
	CHECK(page_is(at, 0, 0) && page_is(at, 1, 0));
	fill_page(at, 1, 0x11);
	CHECK(page_is(at, 1, 0x11) && page_is(at, 0, 0));
	CHECK(madvise(at, PAGE, MADV_DONTNEED) == 0 && page_is(at, 0, 0));
	CHECK(madvise(at, 2 * PAGE, MADV_NORMAL) == 0);
	CHECK_ERROR(madvise(at + 1, PAGE, MADV_DONTNEED), EINVAL);
	CHECK_ERROR(madvise(at, PAGE, 9999), EINVAL);
	CHECK(munmap(at, 2 * PAGE) == 0);
}

// What mremap does to the end of a mapping: it shrinks and unmaps the
// tail; it grows in place when the pages after it are free; and without
// MREMAP_MAYMOVE it fails with ENOMEM when they are not. With
// MREMAP_MAYMOVE Linux moves the mapping and carries its contents. Dukovo
// answers every grow with ENOMEM for now (docs/deferred.md), which the C
// libraries' realloc answers by copying, so there either is accepted and
// the case says which it saw.
static void mremap_shrink(void) {
	char *at = map_pages(4);
	if(!at)
		return;
	for(size_t page = 0; page < 4; page++)
		fill_page(at, page, (char)(0x10 + page));
	void *same = mremap(at, 4 * PAGE, 2 * PAGE, 0);
	CHECK(same == at);
	CHECK(page_is(at, 0, 0x10) && page_is(at, 1, 0x11));
	CHECK(faults(at + 2 * PAGE) && faults(at + 3 * PAGE));
	CHECK(munmap(at, 2 * PAGE) == 0);
}

// Two pages grown to four, with the two pages after them free, stay where
// they are, keep their contents, and the new pages read as zeros and can be
// written.
static void mremap_grow(void) {
	char *at = map_pages(4);
	if(!at)
		return;
	fill_page(at, 0, 0x21);
	fill_page(at, 1, 0x22);
	CHECK(munmap(at + 2 * PAGE, 2 * PAGE) == 0);
	void *same = mremap(at, 2 * PAGE, 4 * PAGE, 0);
	if(same == MAP_FAILED && errno == ENOMEM && on_dukovo) {
		note("growing in place failed with ENOMEM");
		CHECK(page_is(at, 0, 0x21) && page_is(at, 1, 0x22));
		CHECK(munmap(at, 2 * PAGE) == 0);
		return;
	}
	CHECK(same == at);
	if(same == at) {
		CHECK(page_is(at, 0, 0x21) && page_is(at, 1, 0x22));
		CHECK(page_is(at, 2, 0) && page_is(at, 3, 0));
		fill_page(at, 3, 0x24);
		CHECK(page_is(at, 3, 0x24));
		CHECK(munmap(at, 4 * PAGE) == 0);
	} else {
		CHECK(munmap(at, 2 * PAGE) == 0);
	}
}

// A page with a free page and then mapped pages after it cannot grow to
// three pages in place, and without MREMAP_MAYMOVE says so.
static void mremap_blocked(void) {
	char *at = map_pages(4);
	if(!at)
		return;
	fill_page(at, 0, 0x31);
	fill_page(at, 2, 0x33);
	CHECK(munmap(at + PAGE, PAGE) == 0);
	CHECK_ERROR(mremap(at, PAGE, 3 * PAGE, 0), ENOMEM);
	CHECK(page_is(at, 0, 0x31) && page_is(at, 2, 0x33));
	void *moved = mremap(at, PAGE, 3 * PAGE, MREMAP_MAYMOVE);
	if(moved == MAP_FAILED) {
		CHECK(errno == ENOMEM && on_dukovo);
		note("MREMAP_MAYMOVE failed with ENOMEM");
		CHECK(munmap(at, PAGE) == 0);
	} else {
		note("MREMAP_MAYMOVE moved the mapping");
		CHECK(moved != (void *)at);
		CHECK(page_is(moved, 0, 0x31) && page_is(moved, 1, 0) && page_is(moved, 2, 0));
		CHECK(faults(at));
		CHECK(munmap(moved, 3 * PAGE) == 0);
	}
	CHECK(munmap(at + 2 * PAGE, 2 * PAGE) == 0);
}

// The three mremap cases: shrinking, growing, and a grow that is blocked.
static void mremap_case(void) {
	mremap_shrink();
	mremap_grow();
	mremap_blocked();
}

// uname answers as Linux does: its sysname and machine are Linux's, and
// each field is there.
static void uname_case(void) {
	struct utsname name;
	CHECK(uname(&name) == 0);
	CHECK(!strcmp(name.sysname, "Linux"));
	CHECK(!strcmp(name.machine, "x86_64"));
	CHECK(name.nodename[0] != 0);
	CHECK(name.release[0] >= '0' && name.release[0] <= '9');
	CHECK(name.version[0] != 0);
	CHECK(name.domainname[0] != 0);
}

// getcwd as the raw call makes it: it gives the length of the path with its
// NUL, fails with ERANGE for a buffer that is one byte too small, and
// follows the directory the program goes to and comes back from.
static void getcwd_case(void) {
	char directory[512], buffer[PATH_MAX], small[PATH_MAX];
	make_directory(directory, sizeof(directory), "cwd");
	char deeper[PATH_MAX];
	snprintf(deeper, sizeof(deeper), "%s/inner", directory);
	CHECK(mkdir(deeper, 0755) == 0);
	CHECK(chdir(deeper) == 0);
	long length = syscall(SYS_getcwd, buffer, sizeof(buffer));
	CHECK(length > 0 && (size_t)length == strlen(buffer) + 1);
	CHECK(!strcmp(buffer + strlen(buffer) - strlen("/cwd/inner"), "/cwd/inner"));
	if(length > 1) {
		CHECK(syscall(SYS_getcwd, small, (size_t)length) == length);
		CHECK(!strcmp(small, buffer));
		CHECK_ERROR(syscall(SYS_getcwd, small, (size_t)length - 1), ERANGE);
	}
	CHECK_ERROR(syscall(SYS_getcwd, small, 1UL), ERANGE);
	CHECK(chdir("..") == 0);
	length = syscall(SYS_getcwd, buffer, sizeof(buffer));
	CHECK(length > 0 && (size_t)length == strlen(buffer) + 1);
	CHECK(!strcmp(buffer + strlen(buffer) - strlen("/cwd"), "/cwd"));
	CHECK(chdir(start_dir) == 0);
	CHECK(syscall(SYS_getcwd, buffer, sizeof(buffer)) > 0 && !strcmp(buffer, start_dir));
}

// getdents64, whose entries are the kernel's own records.
struct linux_dirent64 {
	uint64_t d_ino;
	int64_t d_off;
	unsigned short d_reclen;
	unsigned char d_type;
	char d_name[];
};

struct listing {
	int count;
	char names[16][64];
	unsigned char types[16];
};

// Reads directory `fd` to its end with the raw call, into a buffer so small
// that it takes several calls, and puts each entry in `listing`, checking
// that each record is a multiple of 8 bytes long and holds its name.
static void list_raw(int fd, struct listing *listing) {
	_Alignas(8) char buffer[96];
	listing->count = 0;
	for(;;) {
		long got = syscall(SYS_getdents64, fd, buffer, sizeof(buffer));
		CHECK(got >= 0);
		if(got <= 0)
			return;
		for(long at = 0; at < got;) {
			struct linux_dirent64 *entry = (struct linux_dirent64 *)(buffer + at);
			CHECK(entry->d_reclen % 8 == 0);
			CHECK(entry->d_reclen >= 19 + strlen(entry->d_name) + 1);
			CHECK(entry->d_ino != 0);
			if(listing->count < 16) {
				snprintf(listing->names[listing->count], sizeof(listing->names[0]), "%s",
						entry->d_name);
				listing->types[listing->count] = entry->d_type;
				listing->count++;
			}
			at += entry->d_reclen;
		}
	}
}

// A directory with three files lists "." and "..", the three, each once;
// a file has type DT_REG and the directories DT_DIR; after a seek to the
// start the same list comes again; a buffer too small for any entry, and a
// file where a directory is wanted, are refused.
static void getdents64_case(void) {
	char directory[512], path[PATH_MAX];
	make_directory(directory, sizeof(directory), "entries");
	static const char *const FILES[] = {"a", "bb", "ccc"};
	for(int i = 0; i < 3; i++) {
		snprintf(path, sizeof(path), "%s/%s", directory, FILES[i]);
		make_file(path, 10, 'x', 0644);
	}
	int fd = open(directory, O_RDONLY | O_DIRECTORY);
	CHECK(fd >= 0);
	if(fd < 0)
		return;
	struct listing first, second;
	list_raw(fd, &first);
	CHECK(first.count == 5);
	int seen_dot = 0, seen_dotdot = 0, seen[3] = {0, 0, 0};
	for(int i = 0; i < first.count; i++) {
		const char *name = first.names[i];
		if(!strcmp(name, ".")) {
			seen_dot++;
			CHECK(first.types[i] == DT_DIR);
		} else if(!strcmp(name, "..")) {
			seen_dotdot++;
			CHECK(first.types[i] == DT_DIR);
		} else {
			for(int j = 0; j < 3; j++)
				if(!strcmp(name, FILES[j])) {
					seen[j]++;
					CHECK(first.types[i] == DT_REG);
				}
		}
	}
	CHECK(seen_dot == 1 && seen_dotdot == 1);
	CHECK(seen[0] == 1 && seen[1] == 1 && seen[2] == 1);
	CHECK(lseek(fd, 0, SEEK_SET) == 0);
	list_raw(fd, &second);
	CHECK(second.count == first.count);
	for(int i = 0; i < first.count && i < second.count; i++)
		CHECK(!strcmp(first.names[i], second.names[i]));
	char tiny[4];
	CHECK(lseek(fd, 0, SEEK_SET) == 0);
	CHECK_ERROR(syscall(SYS_getdents64, fd, tiny, sizeof(tiny)), EINVAL);
	CHECK(close(fd) == 0);
	snprintf(path, sizeof(path), "%s/a", directory);
	fd = open(path, O_RDONLY);
	CHECK(fd >= 0);
	CHECK_ERROR(syscall(SYS_getdents64, fd, first.names, sizeof(first.names)), ENOTDIR);
	CHECK(close(fd) == 0);
}

// statx as the raw call makes it: the basic statistics are all given and a
// file's size, type, mode, link count and owner are right; the same
// through a descriptor with AT_EMPTY_PATH, which newfstatat takes too and
// answers as fstat does; a name that is not there is ENOENT.
static void statx_case(void) {
	char directory[512], path[PATH_MAX];
	make_directory(directory, sizeof(directory), "status");
	snprintf(path, sizeof(path), "%s/file", directory);
	make_file(path, 12345, 'q', 0640);
	struct linux_statx stx;
	memset(&stx, 0xff, sizeof(stx));
	CHECK(syscall(SYS_statx, (long)AT_FDCWD, path, 0L, (long)LINUX_STATX_BASIC_STATS, &stx) ==
			0);
	CHECK((stx.stx_mask & LINUX_STATX_BASIC_STATS) == LINUX_STATX_BASIC_STATS);
	CHECK(stx.stx_size == 12345);
	CHECK(S_ISREG(stx.stx_mode) && (stx.stx_mode & 07777) == 0640);
	CHECK(stx.stx_nlink == 1);
	CHECK(stx.stx_uid == getuid());
	CHECK(stx.stx_ino != 0 && stx.stx_blksize != 0);
	struct linux_statx dir;
	CHECK(syscall(SYS_statx, (long)AT_FDCWD, directory, 0L, (long)LINUX_STATX_BASIC_STATS, &dir) ==
			0);
	CHECK(S_ISDIR(dir.stx_mode));
	int fd = open(path, O_RDONLY);
	CHECK(fd >= 0);
	struct linux_statx by_fd;
	memset(&by_fd, 0xff, sizeof(by_fd));
	CHECK(syscall(SYS_statx, (long)fd, "", (long)AT_EMPTY_PATH, (long)LINUX_STATX_BASIC_STATS,
			    &by_fd) == 0);
	CHECK(by_fd.stx_size == 12345 && by_fd.stx_ino == stx.stx_ino);
	struct stat plain, fstatted;
	memset(&plain, 0xff, sizeof(plain));
	CHECK(syscall(SYS_newfstatat, (long)fd, "", &plain, (long)AT_EMPTY_PATH) == 0);
	CHECK(fstat(fd, &fstatted) == 0);
	CHECK(plain.st_size == 12345 && plain.st_size == fstatted.st_size);
	CHECK(plain.st_mode == fstatted.st_mode && (plain.st_mode & 07777) == 0640);
	CHECK(plain.st_ino == fstatted.st_ino && plain.st_nlink == 1);
	CHECK(close(fd) == 0);
	char missing[PATH_MAX];
	snprintf(missing, sizeof(missing), "%s/missing", directory);
	CHECK_ERROR(syscall(SYS_statx, (long)AT_FDCWD, missing, 0L, (long)LINUX_STATX_BASIC_STATS,
			    &stx),
			ENOENT);
}

// Signals. SIGUSR1 is sent to the thread that waits, and to it alone, by a
// helper thread (tgkill and no timer: kill, tgkill and nanosleep are what a
// Linux program is always given). The helper then writes a byte to a pipe
// the waiting thread reads.

static volatile sig_atomic_t signal_hits;
static volatile sig_atomic_t signal_seen;

// Notes that the signal came, and which one.
static void on_signal(int signal_number, siginfo_t *info, void *context) {
	(void)context;
	signal_seen = info->si_signo == signal_number ? signal_number : -1;
	signal_hits++;
}

struct helper {
	int fd;
	pthread_t target;
	// When, after the helper starts, it signals `target` and when it
	// writes the byte, in milliseconds.
	long signal_ms;
	long write_ms;
};

// What the helper does: signals the waiting thread, then writes the byte.
static void *signal_then_write(void *argument) {
	struct helper *help = argument;
	sleep_ms(help->signal_ms);
	if(pthread_kill(help->target, SIGUSR1) != 0)
		return (void *)1;
	sleep_ms(help->write_ms - help->signal_ms);
	if(write(help->fd, "x", 1) != 1)
		return (void *)1;
	return NULL;
}

// A read blocked on a pipe is cut short by SIGUSR1 one second in. With
// SA_RESTART the handler runs, the read starts again and gets the byte the
// helper writes at 1.5 s; the handler's return goes through the C
// library's restorer, which calls rt_sigreturn, and the program goes on
// where it was.
static void signal_restart(void) {
	int fds[2];
	CHECK(pipe(fds) == 0);
	struct sigaction act, old;
	memset(&act, 0, sizeof(act));
	act.sa_sigaction = on_signal;
	act.sa_flags = SA_RESTART | SA_SIGINFO;
	sigemptyset(&act.sa_mask);
	CHECK(sigaction(SIGUSR1, &act, &old) == 0);
	signal_hits = 0;
	signal_seen = 0;
	struct helper help = {fds[1], pthread_self(), 1000, 1500};
	pthread_t helper;
	long long before = now_ns(CLOCK_MONOTONIC);
	CHECK(pthread_create(&helper, NULL, signal_then_write, &help) == 0);
	char byte = 0;
	ssize_t got = read(fds[0], &byte, 1);
	long long took = now_ns(CLOCK_MONOTONIC) - before;
	CHECK(got == 1 && byte == 'x');
	CHECK(signal_hits == 1 && signal_seen == SIGUSR1);
	CHECK(took >= 1400 * 1000000LL);
	void *result = NULL;
	CHECK(pthread_join(helper, &result) == 0 && result == NULL);
	CHECK(sigaction(SIGUSR1, &old, NULL) == 0);
	close(fds[0]);
	close(fds[1]);
}

// Without SA_RESTART the same read fails with EINTR once the handler has
// run, and the byte the helper writes later is still in the pipe.
static void signal_interrupts(void) {
	int fds[2];
	CHECK(pipe(fds) == 0);
	struct sigaction act, old;
	memset(&act, 0, sizeof(act));
	act.sa_sigaction = on_signal;
	act.sa_flags = SA_SIGINFO;
	sigemptyset(&act.sa_mask);
	CHECK(sigaction(SIGUSR1, &act, &old) == 0);
	signal_hits = 0;
	struct helper help = {fds[1], pthread_self(), 500, 1200};
	pthread_t helper;
	CHECK(pthread_create(&helper, NULL, signal_then_write, &help) == 0);
	char byte = 0;
	CHECK_ERROR(read(fds[0], &byte, 1), EINTR);
	CHECK(signal_hits == 1);
	CHECK(read(fds[0], &byte, 1) == 1 && byte == 'x');
	void *result = NULL;
	CHECK(pthread_join(helper, &result) == 0 && result == NULL);
	CHECK(sigaction(SIGUSR1, &old, NULL) == 0);
	close(fds[0]);
	close(fds[1]);
}

// The signal calls take the size of the signal sets, which is 8 bytes for
// 64 signals, and refuse another.
static void signal_set_size(void) {
	sigset_t set;
	sigemptyset(&set);
	CHECK_ERROR(syscall(SYS_rt_sigprocmask, (long)SIG_BLOCK, &set, NULL, 4L), EINVAL);
	CHECK(syscall(SYS_rt_sigprocmask, (long)SIG_BLOCK, &set, NULL, 8L) == 0);
	CHECK_ERROR(syscall(SYS_rt_sigaction, (long)SIGUSR1, NULL, NULL, 4L), EINVAL);
}

// The interval timers, made as raw calls, since glibc's alarm is the alarm
// call and musl's is setitimer. A timer set for 200 ms reads back as set,
// and reading it leaves it set; alarm returns what was left of it rounded
// to a second, never 0 while it was set, and alarm(0) turns it off. A
// timer that is not one of the three, or a time that is not valid, is
// EINVAL. A timer of 20 ms that runs again every 20 ms sends SIGALRM
// again and again, ending a pause each time, until it is turned off,
// which reads its interval back.
static void signal_timers(void) {
	struct sigaction act, old;
	memset(&act, 0, sizeof(act));
	act.sa_sigaction = on_signal;
	act.sa_flags = SA_SIGINFO;
	sigemptyset(&act.sa_mask);
	CHECK(sigaction(SIGALRM, &act, &old) == 0);
	signal_hits = 0;
	signal_seen = 0;
	struct itimerval once = {{0, 0}, {0, 200000}};
	struct itimerval got;
	CHECK(syscall(SYS_setitimer, (long)ITIMER_REAL, &once, NULL) == 0);
	CHECK(syscall(SYS_getitimer, (long)ITIMER_REAL, &got) == 0);
	CHECK(got.it_value.tv_sec == 0 && got.it_value.tv_usec > 0 &&
			got.it_value.tv_usec <= 200000);
	CHECK(got.it_interval.tv_sec == 0 && got.it_interval.tv_usec == 0);
	CHECK(syscall(SYS_alarm, 0L) == 1);
	CHECK(syscall(SYS_getitimer, (long)ITIMER_REAL, &got) == 0);
	CHECK(got.it_value.tv_sec == 0 && got.it_value.tv_usec == 0);
	CHECK(syscall(SYS_alarm, 5L) == 0);
	CHECK(syscall(SYS_alarm, 0L) == 5);
	CHECK_ERROR(syscall(SYS_setitimer, 3L, &once, NULL), EINVAL);
	CHECK_ERROR(syscall(SYS_getitimer, -1L, &got), EINVAL);
	struct itimerval bad = {{0, 0}, {0, 1000000}};
	CHECK_ERROR(syscall(SYS_setitimer, (long)ITIMER_REAL, &bad, NULL), EINVAL);
	struct itimerval every = {{0, 20000}, {0, 20000}};
	CHECK(syscall(SYS_setitimer, (long)ITIMER_REAL, &every, NULL) == 0);
	long long before = now_ns(CLOCK_MONOTONIC);
	CHECK_ERROR(syscall(SYS_pause), EINTR);
	CHECK(signal_hits >= 1);
	while(signal_hits < 3 && now_ns(CLOCK_MONOTONIC) - before < 5000000000LL)
		sleep_ms(10);
	struct itimerval off = {{0, 0}, {0, 0}};
	CHECK(syscall(SYS_setitimer, (long)ITIMER_REAL, &off, &got) == 0);
	CHECK(got.it_interval.tv_sec == 0 && got.it_interval.tv_usec == 20000);
	CHECK(signal_hits >= 3 && signal_seen == SIGALRM);
	CHECK(sigaction(SIGALRM, &old, NULL) == 0);
}

// The four signal cases.
static void signals(void) {
	signal_restart();
	signal_interrupts();
	signal_set_size();
	signal_timers();
}

// The futex calls with a bit set and an absolute timeout: a wait on the
// monotonic clock that nothing wakes ends with ETIMEDOUT no sooner than its
// time, one whose time has passed ends at once, FUTEX_CLOCK_REALTIME
// reads the realtime clock, and FUTEX_WAKE_BITSET wakes a waiter.

// The futex call with its bit set, made without the C library.
static long futex_call(int *word, int operation, int value, const struct timespec *timeout,
		unsigned bits) {
	return syscall(SYS_futex, word, operation, value, timeout, NULL, bits);
}

struct waiter {
	int word;
	atomic_int entering;
	long result;
	int error;
};

// A thread that waits on a futex word for up to ten seconds, and keeps what
// the call answered.
static void *wait_for_wake(void *argument) {
	struct waiter *waiter = argument;
	struct timespec limit = timespec_of(now_ns(CLOCK_MONOTONIC) + 10000000000LL);
	atomic_store(&waiter->entering, 1);
	waiter->result = futex_call(&waiter->word, FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG, 0, &limit,
			FUTEX_BITSET_MATCH_ANY);
	waiter->error = errno;
	return NULL;
}

// WAIT_BITSET with absolute deadlines ends as the comment above says, and a
// WAKE_BITSET wakes a waiter.
static void futex_bitset(void) {
	static int word;
	word = 0;
	long long start = now_ns(CLOCK_MONOTONIC);
	struct timespec at = timespec_of(start + 50 * 1000000LL);
	errno = 0;
	long result = futex_call(&word, FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG, 0, &at,
			FUTEX_BITSET_MATCH_ANY);
	long long took = now_ns(CLOCK_MONOTONIC) - start;
	CHECK(result == -1 && errno == ETIMEDOUT);
	CHECK(took >= 50 * 1000000LL && took < 2000 * 1000000LL);
	at = timespec_of(now_ns(CLOCK_MONOTONIC) - 1000000LL);
	start = now_ns(CLOCK_MONOTONIC);
	CHECK_ERROR(futex_call(&word, FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG, 0, &at,
			    FUTEX_BITSET_MATCH_ANY),
			ETIMEDOUT);
	CHECK(now_ns(CLOCK_MONOTONIC) - start < 1000 * 1000000LL);
	at = timespec_of(now_ns(CLOCK_REALTIME) + 50 * 1000000LL);
	start = now_ns(CLOCK_MONOTONIC);
	CHECK_ERROR(futex_call(&word, FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME, 0,
			    &at, FUTEX_BITSET_MATCH_ANY),
			ETIMEDOUT);
	CHECK(now_ns(CLOCK_MONOTONIC) - start >= 45 * 1000000LL);
	// A word that is not what the caller expects is not waited on.
	CHECK_ERROR(futex_call(&word, FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG, 1, &at,
			    FUTEX_BITSET_MATCH_ANY),
			EAGAIN);

	static struct waiter waiter;
	waiter.word = 0;
	waiter.result = -2;
	atomic_store(&waiter.entering, 0);
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, wait_for_wake, &waiter) == 0);
	while(!atomic_load(&waiter.entering))
		sleep_ms(1);
	// The waiter may not have entered the call yet: wake until one is woken.
	long woken = 0;
	for(int tries = 0; tries < 300 && woken == 0; tries++) {
		sleep_ms(10);
		woken = futex_call(&waiter.word, FUTEX_WAKE_BITSET | FUTEX_PRIVATE_FLAG, 1, NULL,
				FUTEX_BITSET_MATCH_ANY);
	}
	CHECK(woken == 1);
	CHECK(pthread_join(thread, NULL) == 0);
	CHECK(waiter.result == 0);
}

// A futex call with a second word and a second count, made without the C
// library: the count goes where the timeout would.
static long futex_two(int *word, int operation, int count, long count2, int *second,
		unsigned value) {
	return syscall(SYS_futex, word, operation | FUTEX_PRIVATE_FLAG, count, (void *)count2, second,
			value);
}

// How many threads wait on a futex word, with FUTEX_PRIVATE_FLAG or not as
// `private` says: a requeue of the word to itself that wakes none leaves
// them all where they are, and counts them.
static long futex_waiters_as(int *word, int private) {
	return syscall(SYS_futex, word, FUTEX_REQUEUE | private, 0, (void *)(long)INT_MAX, word, 0);
}

static long futex_waiters(int *word) {
	return futex_waiters_as(word, FUTEX_PRIVATE_FLAG);
}

// Waits until `count` threads wait on `word`, for up to three seconds;
// whether they do.
static int until_waiting_as(int *word, long count, int private) {
	for(int tries = 0; tries < 3000; tries++) {
		if(futex_waiters_as(word, private) == count)
			return 1;
		sleep_ms(1);
	}
	return 0;
}

static int until_waiting(int *word, long count) {
	return until_waiting_as(word, count, FUTEX_PRIVATE_FLAG);
}

// A thread that waits on a futex word that holds `expected`, with the bit
// set `bits`, for up to ten seconds, and keeps what the call answered: as
// a private futex, or as a shared one if `private` is 0.
struct sleeper {
	int *word;
	int expected;
	unsigned bits;
	int private;
	pthread_t thread;
	atomic_int done;
	long result;
	int error;
};

static void *sleep_on(void *argument) {
	struct sleeper *sleeper = argument;
	struct timespec limit = timespec_of(now_ns(CLOCK_MONOTONIC) + 10000000000LL);
	sleeper->result = futex_call(sleeper->word, FUTEX_WAIT_BITSET | sleeper->private,
			sleeper->expected, &limit, sleeper->bits);
	sleeper->error = errno;
	atomic_store(&sleeper->done, 1);
	return NULL;
}

// Starts `sleeper` waiting on `word` while it holds `expected`, with
// FUTEX_PRIVATE_FLAG or not as `private` says.
static void start_sleeper_as(struct sleeper *sleeper, int *word, int expected, unsigned bits,
		int private) {
	sleeper->word = word;
	sleeper->expected = expected;
	sleeper->bits = bits;
	sleeper->private = private;
	sleeper->result = -2;
	sleeper->error = 0;
	atomic_store(&sleeper->done, 0);
	CHECK(pthread_create(&sleeper->thread, NULL, sleep_on, sleeper) == 0);
}

// Starts `sleeper` waiting on `word` as a private futex.
static void start_sleeper(struct sleeper *sleeper, int *word, int expected, unsigned bits) {
	start_sleeper_as(sleeper, word, expected, bits, FUTEX_PRIVATE_FLAG);
}

// Joins `sleeper`, and checks that a wake, not its time or an error, ended
// its wait.
static void check_woken(struct sleeper *sleeper) {
	CHECK(pthread_join(sleeper->thread, NULL) == 0);
	CHECK(sleeper->result == 0);
}

// Bit sets: a wake wakes only the waiters whose bit sets share a bit with
// its own, and a set of no bits is EINVAL for a wait and a wake.
static void futex_sets(void) {
	static int word;
	word = 0;
	CHECK_ERROR(futex_call(&word, FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG, 0, NULL, 0), EINVAL);
	CHECK_ERROR(futex_call(&word, FUTEX_WAKE_BITSET | FUTEX_PRIVATE_FLAG, 1, NULL, 0), EINVAL);
	static struct sleeper one, two;
	start_sleeper(&one, &word, 0, 1);
	start_sleeper(&two, &word, 0, 2 | 4);
	CHECK(until_waiting(&word, 2));
	CHECK(futex_call(&word, FUTEX_WAKE_BITSET | FUTEX_PRIVATE_FLAG, INT_MAX, NULL, 8) == 0);
	CHECK(futex_call(&word, FUTEX_WAKE_BITSET | FUTEX_PRIVATE_FLAG, INT_MAX, NULL, 4) == 1);
	check_woken(&two);
	CHECK(!atomic_load(&one.done));
	CHECK(futex_waiters(&word) == 1);
	CHECK(futex_call(&word, FUTEX_WAKE | FUTEX_PRIVATE_FLAG, INT_MAX, NULL, 0) == 1);
	check_woken(&one);
}

// Requeues: FUTEX_CMP_REQUEUE checks the word first (EAGAIN), negative
// counts are EINVAL, moved waiters wait on the second word without waking,
// and the call returns how many it woke and moved together.
static void futex_requeue(void) {
	static int first, second;
	first = 0;
	second = 0;
	static struct sleeper sleepers[3];
	for(int i = 0; i < 3; i++)
		start_sleeper(&sleepers[i], &first, 0, FUTEX_BITSET_MATCH_ANY);
	CHECK(until_waiting(&first, 3));
	CHECK_ERROR(futex_two(&first, FUTEX_CMP_REQUEUE, 0, INT_MAX, &second, 1), EAGAIN);
	CHECK_ERROR(futex_two(&first, FUTEX_CMP_REQUEUE, -1, 1, &second, 0), EINVAL);
	CHECK_ERROR(futex_two(&first, FUTEX_REQUEUE, 1, -1, &second, 0), EINVAL);
	CHECK(futex_waiters(&first) == 3);
	// None woken, two moved.
	CHECK(futex_two(&first, FUTEX_CMP_REQUEUE, 0, 2, &second, 0) == 2);
	CHECK(futex_waiters(&first) == 1);
	CHECK(futex_waiters(&second) == 2);
	for(int i = 0; i < 3; i++)
		CHECK(!atomic_load(&sleepers[i].done));
	// One woken on the second word, one moved back.
	CHECK(futex_two(&second, FUTEX_REQUEUE, 1, INT_MAX, &first, 0) == 2);
	CHECK(futex_waiters(&second) == 0);
	CHECK(futex_waiters(&first) == 2);
	CHECK(futex_call(&first, FUTEX_WAKE | FUTEX_PRIVATE_FLAG, INT_MAX, NULL, 0) == 2);
	for(int i = 0; i < 3; i++)
		check_woken(&sleepers[i]);
}

// FUTEX_WAKE_OP: the operation changes the second word, the first word's
// waiters wake, and the second's only if the comparison of its old value
// holds; an unknown operation is ENOSYS, and a second word that cannot be
// written EFAULT.
static void futex_wake_op(void) {
	static int first, second;
	first = 0;
	second = 5;
	CHECK_ERROR(futex_two(&first, FUTEX_WAKE_OP, 1, 1, &second, FUTEX_OP(7, 0, 0, 0)),
			ENOSYS);
	CHECK_ERROR(futex_two(&first, FUTEX_WAKE_OP, 1, 1, NULL, FUTEX_OP(FUTEX_OP_SET, 0, 0, 0)),
			EFAULT);
	CHECK(second == 5);
	static struct sleeper on_first, on_second;
	start_sleeper(&on_first, &first, 0, FUTEX_BITSET_MATCH_ANY);
	start_sleeper(&on_second, &second, 5, FUTEX_BITSET_MATCH_ANY);
	CHECK(until_waiting(&first, 1) && until_waiting(&second, 1));
	// 5 + 2, and 5 > 4: both wake.
	CHECK(futex_two(&first, FUTEX_WAKE_OP, 1, 1, &second,
			      FUTEX_OP(FUTEX_OP_ADD, 2, FUTEX_OP_CMP_GT, 4)) == 2);
	CHECK(second == 7);
	check_woken(&on_first);
	check_woken(&on_second);
	start_sleeper(&on_second, &second, 7, FUTEX_BITSET_MATCH_ANY);
	CHECK(until_waiting(&second, 1));
	// 7 is not 0: the word changes and no one wakes.
	CHECK(futex_two(&first, FUTEX_WAKE_OP, 1, 1, &second,
			      FUTEX_OP(FUTEX_OP_OR | FUTEX_OP_OPARG_SHIFT, 4, FUTEX_OP_CMP_EQ, 0)) == 0);
	CHECK(second == (7 | 16));
	CHECK(!atomic_load(&on_second.done));
	CHECK(futex_waiters(&second) == 1);
	CHECK(futex_call(&second, FUTEX_WAKE | FUTEX_PRIVATE_FLAG, 1, NULL, 0) == 1);
	check_woken(&on_second);
}

// The kernel's robust list records, as linux/futex.h has them.
struct linux_robust_list {
	struct linux_robust_list *next;
};

struct linux_robust_list_head {
	struct linux_robust_list list;
	long futex_offset;
	struct linux_robust_list *list_op_pending;
};

// A robust lock: its list entry, and its word.
struct robust_lock {
	struct linux_robust_list entry;
	int word;
};

// What the owner of the robust locks does: takes the locks, names them in
// its robust list, and ends once told to.
static struct robust_locks {
	struct linux_robust_list_head head;
	struct robust_lock waited, held, other, pending;
	atomic_int ready, go;
	int tid;
	long set;
} robust;

static void *robust_owner(void *argument) {
	(void)argument;
	robust.tid = (int)syscall(SYS_gettid);
	robust.waited.word = robust.tid | (int)FUTEX_WAITERS;
	robust.held.word = robust.tid;
	robust.other.word = 77;
	robust.pending.word = 0;
	robust.head.futex_offset = (long)offsetof(struct robust_lock, word);
	robust.head.list.next = &robust.waited.entry;
	robust.waited.entry.next = &robust.held.entry;
	robust.held.entry.next = &robust.other.entry;
	robust.other.entry.next = &robust.head.list;
	robust.head.list_op_pending = &robust.pending.entry;
	robust.set = syscall(SYS_set_robust_list, &robust.head, sizeof(robust.head));
	atomic_store(&robust.ready, 1);
	while(!atomic_load(&robust.go))
		sleep_ms(1);
	return NULL;
}

// A robust list: when its thread ends, each lock word it owns gets
// FUTEX_OWNER_DIED, keeping FUTEX_WAITERS, and a waiter on one with
// FUTEX_WAITERS wakes; a word another thread owns, and a pending entry's
// word that no one owns, are left as they are. A head of another size is
// EINVAL.
static void robust_list(void) {
	CHECK_ERROR(syscall(SYS_set_robust_list, &robust.head, sizeof(robust.head) + 1), EINVAL);
	memset(&robust, 0, sizeof(robust));
	pthread_t owner;
	CHECK(pthread_create(&owner, NULL, robust_owner, NULL) == 0);
	while(!atomic_load(&robust.ready))
		sleep_ms(1);
	CHECK(robust.set == 0);
	// The kernel wakes a robust lock's waiter as a shared futex's, as a C
	// library's robust mutexes wait.
	static struct sleeper sleeper;
	start_sleeper_as(&sleeper, &robust.waited.word, robust.tid | (int)FUTEX_WAITERS,
			FUTEX_BITSET_MATCH_ANY, 0);
	CHECK(until_waiting_as(&robust.waited.word, 1, 0));
	atomic_store(&robust.go, 1);
	CHECK(pthread_join(owner, NULL) == 0);
	check_woken(&sleeper);
	CHECK((unsigned)robust.waited.word == (FUTEX_WAITERS | FUTEX_OWNER_DIED));
	CHECK((unsigned)robust.held.word == FUTEX_OWNER_DIED);
	CHECK(robust.other.word == 77);
	CHECK(robust.pending.word == 0);
}

// What an unprivileged program may not do. io_uring is not there on Dukovo,
// and Linux may or may not have it, with a call that makes no ring; a
// new user namespace is refused on Dukovo and Linux may allow one, in which
// case the case has nothing to check; mount, reboot (with a magic number
// no reboot takes) and sethostname are for a privileged user, so they run
// as an unprivileged one only.
static void refused(void) {
	struct { unsigned char bytes[120]; } params;
	memset(&params, 0, sizeof(params));
	errno = 0;
	long made = syscall(SYS_io_uring_setup, 0L, &params);
	CHECK(made == -1);
	CHECK(on_dukovo ? errno == ENOSYS : (errno == ENOSYS || errno == EPERM || errno == EINVAL));
	if(geteuid() == 0) {
		note("mount, reboot and sethostname are not tried as the superuser");
	} else {
		CHECK_ERROR(syscall(SYS_mount, "linux-core", scratch, "tmpfs", 0L, NULL), EPERM);
		CHECK_ERROR(syscall(SYS_reboot, 0L, 0L, 0L, NULL), EPERM);
		CHECK_ERROR(syscall(SYS_sethostname, "linux-core", 10L), EPERM);
	}
	fflush(stdout);
	long pid = syscall(SYS_clone, (long)(CLONE_NEWUSER | SIGCHLD), 0L, 0L, 0L, 0L);
	if(pid == 0)
		_exit(0);
	if(pid > 0) {
		CHECK(!on_dukovo);
		CHECK_EXITED(pid, 0);
		note("this system allows a new user namespace");
	} else if(on_dukovo) {
		CHECK(errno == EPERM);
	}
}

// The identity calls agree with what the program was started with: the
// auxiliary vector's user and group numbers are the calls', as is the
// real, effective and saved user number, and the page size is 4096. A
// setter that changes nothing succeeds, and one that would change
// something is refused to an unprivileged user.
static void ids(void) {
	errno = 0;
	unsigned long at_uid = getauxval(AT_UID), at_euid = getauxval(AT_EUID);
	unsigned long at_gid = getauxval(AT_GID), at_egid = getauxval(AT_EGID);
	CHECK(errno == 0);
	CHECK(at_uid == getuid() && at_euid == geteuid());
	CHECK(at_gid == getgid() && at_egid == getegid());
	unsigned real, effective, saved;
	CHECK(syscall(SYS_getresuid, &real, &effective, &saved) == 0);
	CHECK(real == getuid() && effective == geteuid());
	CHECK(getauxval(AT_PAGESZ) == PAGE);
	CHECK(getauxval(AT_SECURE) == 0);
	CHECK(getauxval(AT_RANDOM) != 0);
	CHECK(getgroups(0, NULL) >= 0);
	if(geteuid() != 0) {
		CHECK(setuid(getuid()) == 0);
		CHECK_ERROR(setuid(getuid() + 1), EPERM);
		CHECK_ERROR(setgid(getgid() + 1), EPERM);
	}
}

// New descriptors take the lowest free number: with 0 closed an open gives
// 0, a hole in the numbers is filled before any higher number, F_DUPFD
// takes the lowest from its minimum, dup2 onto itself gives the descriptor
// and dup3 refuses it, and O_CLOEXEC shows in the descriptor's flags.
static void fd_lowest(void) {
	char directory[512], path[PATH_MAX];
	make_directory(directory, sizeof(directory), "descriptors");
	snprintf(path, sizeof(path), "%s/file", directory);
	make_file(path, 1, 'f', 0644);
	int saved = dup(0);
	CHECK(saved >= 3);
	CHECK(close(0) == 0);
	int fd = open(path, O_RDONLY);
	CHECK(fd == 0);
	CHECK(dup2(saved, 0) == 0);
	CHECK(close(saved) == 0);

	int first = open(path, O_RDONLY), second = open(path, O_RDONLY);
	int third = open(path, O_RDONLY);
	CHECK(first >= 0 && second == first + 1 && third == second + 1);
	CHECK(close(second) == 0);
	int again = open(path, O_RDONLY);
	CHECK(again == second);
	CHECK((fcntl(again, F_GETFD) & FD_CLOEXEC) == 0);
	int cloexec = open(path, O_RDONLY | O_CLOEXEC);
	CHECK(cloexec == third + 1);
	CHECK((fcntl(cloexec, F_GETFD) & FD_CLOEXEC) != 0);
	int from_ten = fcntl(first, F_DUPFD, 10);
	CHECK(from_ten >= 10);
	int dup_cloexec = fcntl(first, F_DUPFD_CLOEXEC, 10);
	CHECK(dup_cloexec > from_ten);
	CHECK((fcntl(dup_cloexec, F_GETFD) & FD_CLOEXEC) != 0);
	CHECK(dup2(first, first) == first);
	CHECK_ERROR(dup3(first, first, 0), EINVAL);
	CHECK(dup3(first, third, O_CLOEXEC) == third);
	CHECK((fcntl(third, F_GETFD) & FD_CLOEXEC) != 0);
	int open_fds[] = {first, again, third, cloexec, from_ten, dup_cloexec};
	for(size_t i = 0; i < sizeof(open_fds) / sizeof(open_fds[0]); i++)
		close(open_fds[i]);
}

// umask returns the old mask and sets the new one, which clears bits of the
// mode a file or directory is made with.
static void umask_case(void) {
	char directory[512], path[PATH_MAX];
	make_directory(directory, sizeof(directory), "umask");
	mode_t old = umask(027);
	CHECK(umask(027) == 027);
	snprintf(path, sizeof(path), "%s/file", directory);
	int fd = open(path, O_WRONLY | O_CREAT | O_EXCL, 0666);
	CHECK(fd >= 0);
	struct stat st;
	CHECK(fstat(fd, &st) == 0 && (st.st_mode & 0777) == 0640);
	CHECK(close(fd) == 0);
	snprintf(path, sizeof(path), "%s/folder", directory);
	CHECK(mkdir(path, 0777) == 0);
	CHECK(stat(path, &st) == 0 && (st.st_mode & 0777) == 0750);
	CHECK(umask(0) == 027);
	snprintf(path, sizeof(path), "%s/open", directory);
	fd = open(path, O_WRONLY | O_CREAT | O_EXCL, 0666);
	CHECK(fd >= 0);
	CHECK(fstat(fd, &st) == 0 && (st.st_mode & 0777) == 0666);
	CHECK(close(fd) == 0);
	CHECK(umask(old) == 0);
}

// A pseudo-terminal's settings through the C library's termios calls,
// which move the kernel's shorter struct termios: isatty says yes for the
// terminal and no, with ENOTTY, for a file; a new terminal is canonical and
// echoes; cfmakeraw's settings are kept, and the terminal then hands over
// bytes as they come; tcdrain (TCSBRK) and the window size answer.
static void terminal(void) {
	const char *devices = on_dukovo ? "/devices" : "/dev";
	char path[64];
	snprintf(path, sizeof(path), "%s/ptmx", devices);
	int master = open(path, O_RDWR | O_NOCTTY | O_CLOEXEC);
	if(master < 0) {
		skip("no pseudo-terminals");
		return;
	}
	unsigned number = 0;
	int unlock = 0;
	CHECK(ioctl(master, TIOCGPTN, &number) == 0);
	CHECK(ioctl(master, TIOCSPTLCK, &unlock) == 0);
	snprintf(path, sizeof(path), "%s/pts/%u", devices, number);
	int slave = open(path, O_RDWR | O_NOCTTY | O_CLOEXEC);
	CHECK(slave >= 0);
	if(slave < 0) {
		close(master);
		return;
	}
	CHECK(isatty(slave) == 1);
	char directory[512], file[PATH_MAX];
	make_directory(directory, sizeof(directory), "terminal");
	snprintf(file, sizeof(file), "%s/file", directory);
	make_file(file, 1, 'f', 0644);
	int plain = open(file, O_RDONLY | O_CLOEXEC);
	CHECK(plain >= 0);
	errno = 0;
	CHECK(isatty(plain) == 0 && errno == ENOTTY);
	close(plain);

	struct termios settings;
	CHECK(tcgetattr(slave, &settings) == 0);
	CHECK((settings.c_lflag & (ICANON | ECHO)) == (ICANON | ECHO));
	CHECK(settings.c_cc[VINTR] == 3 && settings.c_cc[VEOF] == 4);
	cfmakeraw(&settings);
	settings.c_cc[VMIN] = 1;
	settings.c_cc[VTIME] = 0;
	CHECK(tcsetattr(slave, TCSANOW, &settings) == 0);
	struct termios again;
	memset(&again, 0, sizeof(again));
	CHECK(tcgetattr(slave, &again) == 0);
	CHECK((again.c_lflag & (ICANON | ECHO | ISIG)) == 0);
	CHECK(again.c_iflag == settings.c_iflag && again.c_oflag == settings.c_oflag);
	CHECK(again.c_cc[VMIN] == 1 && again.c_cc[VTIME] == 0);

	CHECK(write(master, "ab", 2) == 2);
	char got[2];
	size_t have = 0;
	while(have < sizeof(got)) {
		ssize_t n = read(slave, got + have, sizeof(got) - have);
		if(n <= 0) {
			FAIL("read from the raw terminal");
			break;
		}
		have += (size_t)n;
	}
	CHECK(have == 2 && memcmp(got, "ab", 2) == 0);
	CHECK(tcdrain(slave) == 0);
	struct winsize size;
	CHECK(ioctl(slave, TIOCGWINSZ, &size) == 0);
	close(slave);
	close(master);
}

struct test_case {
	const char *name;
	void (*run)(void);
};

static const struct test_case CASES[] = {
	{"threads", threads},
	{"gettid", gettid_case},
	{"fork", fork_raw},
	{"vfork", vfork_case},
	{"execve_cwd", execve_cwd},
	{"exec_native", exec_native},
	{"native_cwd", native_cwd},
	{"started_in", started_in_case},
	{"arch_prctl", arch_prctl_case},
	{"brk", brk_case},
	{"madvise_dontneed", madvise_dontneed},
	{"mremap", mremap_case},
	{"uname", uname_case},
	{"getcwd", getcwd_case},
	{"getdents64", getdents64_case},
	{"statx", statx_case},
	{"signals", signals},
	{"futex_bitset", futex_bitset},
	{"futex_sets", futex_sets},
	{"futex_requeue", futex_requeue},
	{"futex_wake_op", futex_wake_op},
	{"robust_list", robust_list},
	{"refused", refused},
	{"ids", ids},
	{"fd_lowest", fd_lowest},
	{"umask", umask_case},
	{"terminal", terminal},
};

enum { CASE_COUNT = sizeof(CASES) / sizeof(CASES[0]) };

// Ends the program when a case takes longer than CASE_LIMIT_SECONDS. Every
// signal is blocked in this thread, so that none of them is ever its to
// take.
static void *watchdog(void *argument) {
	(void)argument;
	for(;;) {
		sleep_ms(250);
		long long started = atomic_load(&case_started);
		if(started && now_ns(CLOCK_MONOTONIC) - started > CASE_LIMIT_SECONDS * 1000000000LL) {
			printf("linux-core: %s: FAILED: it took more than %d seconds\n", case_name,
					CASE_LIMIT_SECONDS);
			fflush(stdout);
			_exit(3);
		}
	}
	return NULL;
}

// Starts the watchdog thread, with every signal blocked in it.
static void start_watchdog(void) {
	sigset_t all, before;
	sigfillset(&all);
	pthread_sigmask(SIG_BLOCK, &all, &before);
	pthread_t thread;
	int made = pthread_create(&thread, NULL, watchdog, NULL);
	pthread_sigmask(SIG_SETMASK, &before, NULL);
	if(made != 0)
		printf("linux-core: no watchdog: pthread_create failed with %d\n", made);
	else
		pthread_detach(thread);
}

// Removes one entry of this run's directory, deepest first.
static int remove_entry(const char *path, const struct stat *st, int type, struct FTW *where) {
	(void)st;
	(void)type;
	(void)where;
	return remove(path);
}

// Runs case `test` and says how it went; returns whether it passed.
static int run_case(const struct test_case *test) {
	case_name = test->name;
	case_failures = 0;
	case_skipped = 0;
	atomic_store(&case_started, now_ns(CLOCK_MONOTONIC));
	test->run();
	atomic_store(&case_started, 0);
	if(case_failures) {
		printf("linux-core: %s FAILED\n", test->name);
		fflush(stdout);
		return 0;
	}
	if(!case_skipped)
		printf("linux-core: %s ok\n", test->name);
	fflush(stdout);
	return 1;
}

// Finds this program's path from `argv0`, which names it as the shell that
// ran it did: a path, absolute or from the working directory.
static int find_self(const char *argv0) {
	if(argv0[0] == '/')
		snprintf(self_path, sizeof(self_path), "%s", argv0);
	else if(strchr(argv0, '/'))
		snprintf(self_path, sizeof(self_path), "%s/%s", start_dir, argv0);
	else
		return -1;
	return 0;
}

// Runs the cases the arguments name, or all of them, and ends with how many
// passed, skipped or failed.
int main(int argc, char **argv) {
	if(argc == 3 && !strcmp(argv[1], "--exit"))
		_exit(atoi(argv[2]));
	if(argc == 2 && !strcmp(argv[1], "--cwd")) {
		char directory[PATH_MAX];
		if(!getcwd(directory, sizeof(directory)))
			return 1;
		printf("%s\n", directory);
		return 0;
	}
	setvbuf(stdout, NULL, _IOLBF, 0);
	if(!getcwd(start_dir, sizeof(start_dir))) {
		printf("linux-core: cannot find the working directory: %s\n", strerror(errno));
		return 2;
	}
	if(find_self(argv[0]) != 0) {
		printf("linux-core: run it by its path, which the cases run again\n");
		return 2;
	}
	int selected[CASE_COUNT] = {0};
	int some_selected = 0;
	for(int i = 1; i < argc; i++) {
		if(i + 1 < argc && !strcmp(argv[i], "--native")) {
			native_program = argv[++i];
		} else if(i + 1 < argc && !strcmp(argv[i], "--native-pwd")) {
			native_pwd_program = argv[++i];
		} else if(i + 1 < argc && !strcmp(argv[i], "--started-in")) {
			started_in = argv[++i];
		} else {
			int found = 0;
			for(int j = 0; j < CASE_COUNT; j++)
				if(!strcmp(argv[i], CASES[j].name))
					selected[j] = found = some_selected = 1;
			if(!found) {
				printf("linux-core: there is no case named %s\n", argv[i]);
				return 2;
			}
		}
	}
	struct utsname name;
	if(uname(&name) == 0)
		on_dukovo = strstr(name.release, "dukovo") || strstr(name.version, "Dukovo");
	const char *base = getenv("TMPDIR");
	if(!base || !*base)
		base = "/tmp";
	if(strlen(base) + sizeof("/linux-core-XXXXXX") > sizeof(scratch)) {
		printf("linux-core: %s is too long a name for the directory of the files\n", base);
		return 2;
	}
	snprintf(scratch, sizeof(scratch), "%s/linux-core-XXXXXX", base);
	if(!mkdtemp(scratch)) {
		printf("linux-core: cannot make a directory in %s: %s\n", base, strerror(errno));
		return 2;
	}
	printf("linux-core: running on %s\n", on_dukovo ? "Dukovo" : "Linux");
	start_watchdog();
	int ran = 0, failed = 0, skipped = 0;
	for(int i = 0; i < CASE_COUNT; i++) {
		if(some_selected && !selected[i])
			continue;
		ran++;
		if(!run_case(&CASES[i]))
			failed++;
		skipped += case_skipped;
	}
	nftw(scratch, remove_entry, 16, FTW_DEPTH | FTW_PHYS);
	if(failed) {
		printf("linux-core: %d of %d failed\n", failed, ran);
		return 1;
	}
	if(skipped)
		printf("linux-core: all %d passed, %d skipped\n", ran, skipped);
	else
		printf("linux-core: all %d passed\n", ran);
	return 0;
}
