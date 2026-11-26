// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The server benchmarks of the lab's performance tests: one C program,
// built for Radix on mlibc and for Linux with the host's C compiler and
// C library (linux-bench runs it there, in the kit's initramfs), so both
// systems run the same servers through the same driver. It starts each
// server it is given from the applications folder, drives it from this
// process over the loopback interface or a Unix socket, and prints a
// line for each pass of a row in the format of bench.c,
//
//     BENCH name=<name> ns=<nanoseconds per operation> iterations=<count>
//
// A row runs one warm-up pass and then PASSES measured ones; the lab keeps
// the fastest line of a row. After the line or lines of each measured pass
// it prints how the machine spent the pass, summed over all CPUs,
//
//     BENCHCOUNT group=<name> rounds=<operations> switches=<n> user_ns=<n>
//         system_ns=<n> idle_ns=<n> [ipis=<n> halts=<n> timer_writes=<n>
//         preemptions=<n> migrations=<n> faults=<n> polls=<n>
//         mutex_waits=<n> mutex_contended=<n> cap_fills=<n>
//         cap_flushes=<n>] [faults=<n> interrupts=<n>]
//
// from /processes/stat on Radix (all twelve) and from /proc/stat on Linux
// (the first four; the clock ticks become nanoseconds). The Radix fields
// after idle_ns are the IPIs sent, the halts of idle CPUs, the writes to
// the timer, the preemptions (switches away from a thread that could have
// gone on), the migrations (moves of a thread to another CPU than the one
// it ran on last), the page faults resolved, the polls (idle periods a
// wake-up ended while polling), the mutex waits (threads that slept
// waiting for a kernel mutex), the times a thread found a mutex taken, and
// the capability cache's lookups that asked the store and the slots its
// entries were taken from (0 from a kernel without those three). Linux prints instead its page faults
// (pgfault in /proc/vmstat) and its interrupts (the total of the intr
// line of /proc/stat), each left out if its file cannot be read. A run of
// valkey-benchmark prints three rows and one such line, group
// server_valkey_run; if neither file can be read there is no line.
//
// After that line comes the CPU time of the pass's client, the program
// that drove the server (this program for nginx and PHP-FPM, pgbench,
// valkey-benchmark, and the php CLI, which is the whole of its row),
//
//     BENCHCLIENT group=<name> rounds=<operations> user_ns=<n> system_ns=<n>
//
// so that what is left of BENCHCOUNT's user and system time is the
// server's.
//
// With SERVERBENCH_TRACE set in its environment, on Radix, each
// PostgreSQL row runs one more pass against a server started under
// rtrace(1), which counts the system calls of the server and of the
// processes it starts, and prints the table, each line after
// "TRACE <row> ". That pass prints no row: tracing
// slows the server.
//
// With SERVERBENCH_PROFILE set in its environment to anything but an
// empty string, each nginx, PHP-FPM and PostgreSQL row also samples the
// user-mode code of the whole machine during its measured passes (not the
// warm-up) every 500 microseconds of each CPU's time, and after the last
// pass prints where the samples fell, summed over the passes,
//
//     BENCHPROF group=<name> period_ns=<n> passes=<n> user=<n> others=<n>
//         kernel=<n> lost=<n> left_out=<n>
//     BENCHPROF group=<name> object process=<name> object=<file>
//         samples=<n> share=<percent of user>
//     BENCHPROF group=<name> symbol process=<name> object=<file>
//         symbol=<function> samples=<n> share=<percent of user>
//
// so that the lab can lay Radix's profile beside Linux's function by
// function. `user` counts the samples in user mode, `kernel` those in the
// kernel or on an idle CPU, `others` those of other accounts' processes
// that the reader may not see (always 0 on Linux), `lost` those the system
// had no room for and `left_out` those whose lines did not fit Radix's file.
// Every (process, file) pair has an object line, and the 80 busiest
// (process, file, function) triples have a symbol line; a function that is
// not found is `?`, and code in no file (the vDSO, anonymous memory) is
// the object [no file]. A space in a name becomes an underscore. With the
// value `raw` there is also, for every distinct place,
//
//     BENCHPROF group=<name> raw process=<name> path=<path> offset=0x<hex>
//         samples=<n>
//
// where the offset is into the file. Right after the first line of a row
// come its counts, summed over the passes,
//
//     BENCHPROF group=<name> counter name=<name> value=<n>
//     BENCHPROF group=<name> changes process=<name> count=<n>
//
// A counter line is one count of what the machine did in user mode, in the
// order they were first seen. On Radix these are the kernel's own counts
// (such as `tlb_switches_dropped`) and the processor's counters of user
// mode (such as `instructions`, `cycles` and `dtlb_load_walks`, the page
// walks of the TLB); on Linux they are perf counting events of user mode of
// the same names, which the processor's model decides, and the `nr_tlb_`
// lines of /proc/vmstat as `linux_<name>`. A count that the processor had to
// share among more events than it has counters is scaled up to the whole
// time. The names are the same on both systems so that the rows can be
// compared; one that a system lacks is not printed. A changes line is how
// often the address spaces of a process changed their mappings in a way
// that needed a TLB flush, for each process with any, the most first; only
// Radix counts these. Radix reads /processes/profile, which
// holds the samples taken since it was last opened; Linux, which has no
// such file, opens a perf event for each online CPU. The perf events must
// be opened as root, so this happens before the program becomes `bench`;
// where the C library lacks perf_event_open (the Radix build) or perf
// refuses, the program prints "serverbench: no profile: <why>" and runs
// without. Each file's functions are read from its symbol table, or its
// dynamic symbols when it has no table, so a stripped program shows as `?`.
// The samples of a Linux pass are sorted after the pass has been timed,
// not within it.
//
// The servers run as the account `bench` (the program drops root first,
// since PostgreSQL's initdb refuses root), with their data in a folder it
// makes in /tmp and removes at the end, with the servers stopped. A
// failure prints the end of the server's log, then "serverbench: <row>
// failed: <why>", and exits 1.
//
//     serverbench [--applications DIR] nginx|php|valkey|postgresql...
//
// The configurations are written by this program, the same text for each
// system. nginx and PHP-FPM wait for events as their own system gives
// them (poll on Radix, epoll on Linux), and sendfile is off, as in the
// nginx bundle. PostgreSQL's *_flush_after settings are 0 because Radix
// has no sync_file_range (ADR 0521); Linux runs with them 0 too.

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <ftw.h>
#include <grp.h>
#include <netinet/in.h>
#include <poll.h>
#include <pthread.h>
#include <pwd.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/resource.h>
#include <sys/socket.h>
#ifdef __linux__
#include <sys/epoll.h>
#else
#include <sys/event.h>
#endif
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

// Sampling the servers on Linux needs perf_event_open: the header of its
// records and the number of the system call. Radix's C library has neither,
// and -DHAVE_PERF=0 builds without them anywhere.
#ifndef HAVE_PERF
#if defined(__has_include) && __has_include(<linux/perf_event.h>) && __has_include(<sys/syscall.h>)
#include <sys/syscall.h>
#ifdef SYS_perf_event_open
#define HAVE_PERF 1
#endif
#endif
#ifndef HAVE_PERF
#define HAVE_PERF 0
#endif
#endif
#if HAVE_PERF
#if defined(__x86_64__) || defined(__i386__)
#define PERF_CPUID 1
#include <cpuid.h>
#endif
#include <dirent.h>
#include <linux/perf_event.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#endif

#define PASSES 3
#define CONNECTIONS 8

#define NGINX_PORT 18080
#define VALKEY_PORT 16379

#define SMALL_SIZE 1024
#define LARGE_SIZE (1024 * 1024)

// What one measured pass of each row does.
#define NGINX_SMALL_REQUESTS 100000
#define NGINX_CONNECT_REQUESTS 5000
#define NGINX_LARGE_REQUESTS 3000
#define PHP_FPM_REQUESTS 30000
#define PHP_COMPUTE_PASSES 2000
#define VALKEY_REQUESTS 200000
#define VALKEY_PIPELINED_REQUESTS 800000
#define VALKEY_PIPELINE 16
#define PGBENCH_SELECT_PER_CLIENT 10000
#define PGBENCH_TPCB_PER_CLIENT 400

// A server that has not answered after this long has failed to start, and
// one that has not ended after STOP_SECONDS is killed.
#define READY_SECONDS 60
#define STOP_SECONDS 30

// How much of the end of a log a failure prints.
#define TAIL_LINES 20

static uint64_t now_ns(void) {
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (uint64_t)ts.tv_sec * 1000000000u + (uint64_t)ts.tv_nsec;
}

// What the machine had done at a moment, summed over its CPUs: Radix's
// /processes/stat lists one count a line ("ipis: 12"), Linux's /proc/stat
// has the context switches and the CPU times in clock ticks, and none of
// the Radix-only counts; its page faults and interrupts come from
// /proc/vmstat and the intr line, and `have_faults` and `have_interrupts`
// say whether they were read. `valid` is 0 when neither file could be read.
struct counts {
	int valid;
	int radix;
	int have_faults, have_interrupts;
	uint64_t switches, user_ns, system_ns, idle_ns;
	uint64_t ipis, halts, timer_writes;
	uint64_t preemptions, migrations, faults, polls, mutex_waits;
	uint64_t cap_fills, cap_flushes, mutex_contended;
	uint64_t interrupts;
};

// Reads Radix's /processes/stat; false if it lacks one of the twelve.
static int read_radix_counts(struct counts *counts) {
	FILE *file = fopen("/processes/stat", "r");
	if(!file)
		return 0;
	const struct {
		const char *name;
		uint64_t *value;
	} fields[] = {{"switches", &counts->switches}, {"user_ns", &counts->user_ns}, {"system_ns", &counts->system_ns},
	              {"idle_ns", &counts->idle_ns}, {"ipis", &counts->ipis}, {"halts", &counts->halts},
	              {"timer_writes", &counts->timer_writes},
	              {"preemptions", &counts->preemptions}, {"migrations", &counts->migrations}, {"faults", &counts->faults},
	              {"polls", &counts->polls}, {"mutex_waits", &counts->mutex_waits}};
	const unsigned all = (1u << (sizeof(fields) / sizeof(fields[0]))) - 1;
	unsigned found = 0;
	char line[256];
	while(fgets(line, sizeof(line), file)) {
		for(size_t i = 0; i < sizeof(fields) / sizeof(fields[0]); i++) {
			size_t length = strlen(fields[i].name);
			if(!strncmp(line, fields[i].name, length) && line[length] == ':') {
				*fields[i].value = strtoull(line + length + 1, NULL, 10);
				found |= 1u << i;
			}
		}
	}
	fclose(file);
	// Counted since, so read apart: a kernel without them reads as 0.
	const struct {
		const char *name;
		uint64_t *value;
	} more[] = {{"cap_cache_fills", &counts->cap_fills}, {"cap_cache_flushes", &counts->cap_flushes},
	            {"mutex_contended", &counts->mutex_contended}};
	file = fopen("/processes/stat", "r");
	while(file && fgets(line, sizeof(line), file)) {
		for(size_t i = 0; i < sizeof(more) / sizeof(more[0]); i++) {
			size_t length = strlen(more[i].name);
			if(!strncmp(line, more[i].name, length) && line[length] == ':')
				*more[i].value = strtoull(line + length + 1, NULL, 10);
		}
	}
	if(file)
		fclose(file);
	return found == all;
}

// Reads Linux's /proc/stat; false if it lacks `ctxt` or the `cpu ` line.
static int read_linux_counts(struct counts *counts) {
	FILE *file = fopen("/proc/stat", "r");
	if(!file)
		return 0;
	long hz = sysconf(_SC_CLK_TCK);
	if(hz <= 0)
		hz = 100;
	int have_switches = 0, have_cpu = 0;
	char line[512];
	while(fgets(line, sizeof(line), file)) {
		unsigned long long user, nice, system, idle, iowait = 0, irq = 0, softirq = 0;
		if(!strncmp(line, "ctxt ", 5)) {
			counts->switches = strtoull(line + 5, NULL, 10);
			have_switches = 1;
		} else if(!strncmp(line, "cpu ", 4) &&
		          sscanf(line + 4, "%llu %llu %llu %llu %llu %llu %llu", &user, &nice, &system, &idle, &iowait, &irq, &softirq) >= 4) {
			uint64_t ns_per_tick = 1000000000u / (uint64_t)hz;
			counts->user_ns = (user + nice) * ns_per_tick;
			counts->system_ns = (system + irq + softirq) * ns_per_tick;
			counts->idle_ns = (idle + iowait) * ns_per_tick;
			have_cpu = 1;
		}
	}
	fclose(file);
	return have_switches && have_cpu;
}

// Reads the `pgfault` line of Linux's /proc/vmstat; false if there is none.
static int read_linux_faults(struct counts *counts) {
	FILE *file = fopen("/proc/vmstat", "r");
	if(!file)
		return 0;
	int found = 0;
	char line[256];
	while(!found && fgets(line, sizeof(line), file)) {
		if(!strncmp(line, "pgfault ", 8)) {
			counts->faults = strtoull(line + 8, NULL, 10);
			found = 1;
		}
	}
	fclose(file);
	return found;
}

// Reads the total of Linux's `intr` line of /proc/stat (its first number,
// the interrupts of all kinds); false if there is none. The line is long,
// one number for each interrupt, so only its start is read.
static int read_linux_interrupts(struct counts *counts) {
	FILE *file = fopen("/proc/stat", "r");
	if(!file)
		return 0;
	int found = 0;
	char line[64];
	int start = 1;
	while(fgets(line, sizeof(line), file)) {
		size_t length = strlen(line);
		if(start && !strncmp(line, "intr ", 5)) {
			counts->interrupts = strtoull(line + 5, NULL, 10);
			found = 1;
			break;
		}
		start = length > 0 && line[length - 1] == '\n';
	}
	fclose(file);
	return found;
}

static struct counts counts_now(void) {
	struct counts counts;
	memset(&counts, 0, sizeof(counts));
	if(read_radix_counts(&counts)) {
		counts.valid = counts.radix = 1;
		return counts;
	}
	memset(&counts, 0, sizeof(counts));
	counts.valid = read_linux_counts(&counts);
	counts.have_faults = read_linux_faults(&counts);
	counts.have_interrupts = read_linux_interrupts(&counts);
	return counts;
}

// Prints what the machine did between `before` and `after`, as the
// BENCHCOUNT line of a pass of `rounds` operations; nothing if either
// reading failed.
static void print_counts(const char *group, struct counts before, struct counts after, uint64_t rounds) {
	if(!before.valid || !after.valid || before.radix != after.radix)
		return;
	printf("BENCHCOUNT group=%s rounds=%llu switches=%llu user_ns=%llu system_ns=%llu idle_ns=%llu", group, (unsigned long long)rounds,
	       (unsigned long long)(after.switches - before.switches), (unsigned long long)(after.user_ns - before.user_ns),
	       (unsigned long long)(after.system_ns - before.system_ns), (unsigned long long)(after.idle_ns - before.idle_ns));
	if(after.radix) {
		printf(" ipis=%llu halts=%llu timer_writes=%llu", (unsigned long long)(after.ipis - before.ipis),
		       (unsigned long long)(after.halts - before.halts), (unsigned long long)(after.timer_writes - before.timer_writes));
		printf(" preemptions=%llu migrations=%llu faults=%llu polls=%llu mutex_waits=%llu",
		       (unsigned long long)(after.preemptions - before.preemptions), (unsigned long long)(after.migrations - before.migrations),
		       (unsigned long long)(after.faults - before.faults), (unsigned long long)(after.polls - before.polls),
		       (unsigned long long)(after.mutex_waits - before.mutex_waits));
		printf(" mutex_contended=%llu cap_fills=%llu cap_flushes=%llu",
		       (unsigned long long)(after.mutex_contended - before.mutex_contended),
		       (unsigned long long)(after.cap_fills - before.cap_fills),
		       (unsigned long long)(after.cap_flushes - before.cap_flushes));
	} else {
		if(before.have_faults && after.have_faults)
			printf(" faults=%llu", (unsigned long long)(after.faults - before.faults));
		if(before.have_interrupts && after.have_interrupts)
			printf(" interrupts=%llu", (unsigned long long)(after.interrupts - before.interrupts));
	}
	printf("\n");
	fflush(stdout);
}

// The folder the program runs in, and where it keeps its applications.
static char work[] = "/tmp/serverbench.XXXXXX";
static int work_made;
static const char *applications = "/applications";

// The servers that are running, and the logs a failure prints the end of.
struct child {
	const char *name;
	pid_t pid;
	int stop_signal;
};
static struct child children[4];
static const char *logs[8];
static size_t log_count;

// What the servers and tools run with.
static char *environment[5];

// The CPU time of the last program run_tool ran, as its wait gave it.
static uint64_t tool_user_ns, tool_system_ns;

// A time under 10 ns, as the clock row's fraction of a nanosecond, gets
// four decimals, as in bench.c: the lab holds the two systems' clocks to
// 3%, finer than two decimals of 0.2 ns.
static void print_row(const char *name, double ns, uint64_t iterations) {
	printf("BENCH name=%s ns=%.*f iterations=%llu\n", name, ns < 10 ? 4 : 2, ns,
	       (unsigned long long)iterations);
	fflush(stdout);
}

static uint64_t timeval_ns(struct timeval time) {
	return (uint64_t)time.tv_sec * 1000000000u + (uint64_t)time.tv_usec * 1000u;
}

// This program's own CPU time so far, 0 for both if it cannot be read.
static void own_time(uint64_t *user_ns, uint64_t *system_ns) {
	struct rusage usage;
	*user_ns = *system_ns = 0;
	if(getrusage(RUSAGE_SELF, &usage) == 0) {
		*user_ns = timeval_ns(usage.ru_utime);
		*system_ns = timeval_ns(usage.ru_stime);
	}
}

// Prints the CPU time of a pass's client as the BENCHCLIENT line of a pass
// of `rounds` operations.
static void print_client(const char *group, uint64_t user_ns, uint64_t system_ns, uint64_t rounds) {
	printf("BENCHCLIENT group=%s rounds=%llu user_ns=%llu system_ns=%llu\n", group, (unsigned long long)rounds,
	       (unsigned long long)user_ns, (unsigned long long)system_ns);
	fflush(stdout);
}

// Prints the time per operation since `started`.
static void report(const char *name, uint64_t started, uint64_t count) {
	print_row(name, (double)(now_ns() - started) / (double)count, count);
}

// Prints the last TAIL_LINES lines of <work>/<name>.log.
static void print_tail(const char *name) {
	char path[256];
	snprintf(path, sizeof(path), "%s/%s.log", work, name);
	FILE *file = fopen(path, "r");
	if(!file)
		return;
	char text[8192 + 1];
	fseek(file, 0, SEEK_END);
	long size = ftell(file);
	fseek(file, size > 8192 ? size - 8192 : 0, SEEK_SET);
	size_t length = fread(text, 1, 8192, file);
	fclose(file);
	if(length > 0 && text[length - 1] == '\n')
		length--;
	size_t first = length;
	for(int lines = 0; first > 0; first--) {
		if(text[first - 1] == '\n' && ++lines == TAIL_LINES)
			break;
	}
	printf("serverbench: the end of %s.log:\n", name);
	while(first < length) {
		size_t end = first;
		while(end < length && text[end] != '\n')
			end++;
		printf("serverbench: | %.*s\n", (int)(end - first), text + first);
		first = end + 1;
	}
}

// Prints why a row or step failed, after the logs of the servers, and exits;
// `exit` stops the servers and removes the work folder.
static void fail(const char *what, const char *why, ...) __attribute__((noreturn, format(printf, 2, 3)));
static void fail(const char *what, const char *why, ...) {
	for(size_t i = 0; i < log_count; i++)
		print_tail(logs[i]);
	printf("serverbench: %s failed: ", what);
	va_list arguments;
	va_start(arguments, why);
	vprintf(why, arguments);
	va_end(arguments);
	printf("\n");
	fflush(stdout);
	exit(1);
}

// A string made like printf makes one. The program is short-lived and
// never frees these.
static char *format(const char *text, ...) __attribute__((format(printf, 1, 2)));
static char *format(const char *text, ...) {
	va_list arguments;
	va_start(arguments, text);
	int length = vsnprintf(NULL, 0, text, arguments);
	va_end(arguments);
	char *result = malloc((size_t)length + 1);
	if(!result)
		fail("memory", "out of memory");
	va_start(arguments, text);
	vsnprintf(result, (size_t)length + 1, text, arguments);
	va_end(arguments);
	return result;
}

// The sampling profile of SERVERBENCH_PROFILE. The two backends fill one
// table of samples for the row being measured, each sample a process name,
// the file its code came from and the offset in that file, and the same
// code symbolizes and prints it. Strings are kept once, by number.
#define PROFILE_PERIOD_NS 500000
#define PROFILE_SYMBOLS 80
#define PROFILE_KERNEL_SYMBOLS 400
#define NO_ID ((size_t)-1)

static char **strings;
static size_t string_count, string_capacity;

// The number of a string, the same for equal strings; a new one is kept
// the first time.
static size_t intern(const char *text) {
	for(size_t i = 0; i < string_count; i++) {
		if(!strcmp(strings[i], text))
			return i;
	}
	if(string_count == string_capacity) {
		string_capacity = string_capacity ? string_capacity * 2 : 64;
		strings = realloc(strings, string_capacity * sizeof(strings[0]));
		if(!strings)
			fail("memory", "out of memory");
	}
	strings[string_count] = format("%s", text);
	return string_count++;
}

// Counts for keys of three numbers, in a table that grows. `order` is the
// order the keys came in, which settles ties when the counts are sorted.
struct tally_entry {
	uint64_t a, b, c, count, order;
};
struct tally {
	struct tally_entry *entries;
	size_t count, capacity;
	size_t *slots;
	size_t slot_count;
};

static size_t tally_hash(uint64_t a, uint64_t b, uint64_t c) {
	uint64_t hash = a * 0x9e3779b97f4a7c15ull;
	hash = (hash ^ (hash >> 29) ^ b) * 0xbf58476d1ce4e5b9ull;
	hash = (hash ^ (hash >> 32) ^ c) * 0x94d049bb133111ebull;
	return (size_t)(hash ^ (hash >> 31));
}

static void tally_add(struct tally *tally, uint64_t a, uint64_t b, uint64_t c, uint64_t count) {
	if(tally->count * 2 >= tally->slot_count) {
		size_t slot_count = tally->slot_count ? tally->slot_count * 2 : 256;
		size_t *slots = calloc(slot_count, sizeof(slots[0]));
		if(!slots)
			fail("memory", "out of memory");
		for(size_t i = 0; i < tally->count; i++) {
			const struct tally_entry *entry = &tally->entries[i];
			size_t at = tally_hash(entry->a, entry->b, entry->c) & (slot_count - 1);
			while(slots[at])
				at = (at + 1) & (slot_count - 1);
			slots[at] = i + 1;
		}
		free(tally->slots);
		tally->slots = slots;
		tally->slot_count = slot_count;
	}
	size_t at = tally_hash(a, b, c) & (tally->slot_count - 1);
	while(tally->slots[at]) {
		struct tally_entry *entry = &tally->entries[tally->slots[at] - 1];
		if(entry->a == a && entry->b == b && entry->c == c) {
			entry->count += count;
			return;
		}
		at = (at + 1) & (tally->slot_count - 1);
	}
	if(tally->count == tally->capacity) {
		tally->capacity = tally->capacity ? tally->capacity * 2 : 256;
		tally->entries = realloc(tally->entries, tally->capacity * sizeof(tally->entries[0]));
		if(!tally->entries)
			fail("memory", "out of memory");
	}
	tally->entries[tally->count] = (struct tally_entry){a, b, c, count, tally->count};
	tally->slots[at] = ++tally->count;
}

static void tally_clear(struct tally *tally) {
	tally->count = 0;
	if(tally->slots)
		memset(tally->slots, 0, tally->slot_count * sizeof(tally->slots[0]));
}

static int by_count(const void *left, const void *right) {
	const struct tally_entry *x = left, *y = right;
	if(x->count != y->count)
		return x->count > y->count ? -1 : 1;
	return x->order < y->order ? -1 : x->order > y->order;
}

// A copy of a tally's entries, the largest count first.
static struct tally_entry *tally_sorted(const struct tally *tally) {
	struct tally_entry *entries = malloc((tally->count + 1) * sizeof(entries[0]));
	if(!entries)
		fail("memory", "out of memory");
	if(tally->count)
		memcpy(entries, tally->entries, tally->count * sizeof(entries[0]));
	qsort(entries, tally->count, sizeof(entries[0]), by_count);
	return entries;
}

// What the passes of the row being measured have sampled so far: `samples`
// counts (process name, file path, offset in the file) as numbers of
// strings and the offset; `counters` and `changes` count by the number of
// a name alone.
static struct {
	int passes;
	uint64_t user, others, kernel, lost, left_out;
	struct tally samples;
	// Counts by name, in the order first seen, and by process name.
	struct tally counters, changes;
	// Kernel samples by process name and function name, both strings.
	struct tally kernel_symbols;
} profile_row;

static int profile_on, profile_raw, profile_radix;
// Whether a pass has ended whose samples are not in `profile_row` yet; they
// are taken at the next pass or at the print, so that a pass's time is
// not the time of sorting its samples.
static int profile_pending;
static uint64_t profile_period_ns = PROFILE_PERIOD_NS;
// Radix: the text of /processes/profile that ended the last pass.
static char *profile_text;
static size_t no_file_id, unknown_id;
// The process name of a kernel sample taken with no program running.
static size_t kernel_only_id;

// Reads a whole file into a new string, which is null if it cannot be
// opened. /processes/profile can be a MiB, so the buffer grows.
static char *read_all(const char *path) {
	int fd = open(path, O_RDONLY);
	if(fd < 0)
		return NULL;
	size_t capacity = 65536, length = 0;
	char *text = malloc(capacity);
	if(!text)
		fail("memory", "out of memory");
	for(;;) {
		if(length + 1 == capacity) {
			capacity *= 2;
			text = realloc(text, capacity);
			if(!text)
				fail("memory", "out of memory");
		}
		ssize_t count = read(fd, text + length, capacity - 1 - length);
		if(count < 0 && errno == EINTR)
			continue;
		if(count <= 0)
			break;
		length += (size_t)count;
	}
	close(fd);
	text[length] = 0;
	return text;
}

// Keeps the number of a string at an index of a list that grows; an index
// far beyond any the kernel writes is ignored.
static void remember_id(size_t **ids, size_t *count, unsigned long index, size_t id) {
	if(index >= (1ul << 20))
		return;
	if(index >= *count) {
		size_t grown = index + 1 > *count * 2 ? index + 1 : *count * 2;
		*ids = realloc(*ids, grown * sizeof((*ids)[0]));
		if(!*ids)
			fail("memory", "out of memory");
		for(size_t i = *count; i < grown; i++)
			(*ids)[i] = NO_ID;
		*count = grown;
	}
	(*ids)[index] = id;
}

static size_t known_id(const size_t *ids, size_t count, unsigned long index) {
	return index < count && ids[index] != NO_ID ? ids[index] : unknown_id;
}

// Makes the string of a name or path line of /processes/profile, in which a
// newline or a backslash is a backslash and three octal digits, and keeps
// it. The text is changed in place; it only gets shorter.
static size_t intern_escaped(char *text) {
	char *to = text;
	for(const char *from = text; *from; from++) {
		if(from[0] == '\\' && from[1] >= '0' && from[1] <= '7' && from[2] >= '0' && from[2] <= '7' && from[3] >= '0' && from[3] <= '7') {
			*to++ = (char)(((from[1] - '0') << 6) | ((from[2] - '0') << 3) | (from[3] - '0'));
			from += 3;
		} else {
			*to++ = *from;
		}
	}
	*to = 0;
	return intern(text);
}

// Adds what /processes/profile said, whose text this changes, to the row.
// The names and files come before the samples that use them, but all are
// read before the samples are, so that the order does not matter; a line
// of a kind it does not know is left out, so that the kernel may add some.
static void radix_collect(char *text) {
	size_t length = strlen(text);
	for(size_t i = 0; i < length; i++) {
		if(text[i] == '\n')
			text[i] = 0;
	}
	const struct {
		const char *name;
		uint64_t *value;
	} header[] = {{"user", &profile_row.user}, {"others", &profile_row.others}, {"kernel", &profile_row.kernel},
	              {"lost", &profile_row.lost}, {"left_out", &profile_row.left_out}};
	size_t *name_ids = NULL, *file_ids = NULL;
	size_t name_count = 0, file_count = 0;
	for(char *line = text; line < text + length; line += strlen(line) + 1) {
		if(!strncmp(line, "name ", 5) || !strncmp(line, "file ", 5)) {
			char *end;
			unsigned long index = strtoul(line + 5, &end, 10);
			if(end == line + 5 || *end != ' ')
				continue;
			size_t id = intern_escaped(end + 1);
			if(line[0] == 'n')
				remember_id(&name_ids, &name_count, index, id);
			else
				remember_id(&file_ids, &file_count, index, id);
			continue;
		}
		if(!strncmp(line, "counter ", 8)) {
			const char *space = strchr(line + 8, ' ');
			char name[64];
			if(!space || (size_t)(space - (line + 8)) >= sizeof(name))
				continue;
			memcpy(name, line + 8, (size_t)(space - (line + 8)));
			name[space - (line + 8)] = 0;
			tally_add(&profile_row.counters, intern(name), 0, 0, strtoull(space + 1, NULL, 10));
			continue;
		}
		if(!strncmp(line, "period_ns:", 10) && strtoull(line + 10, NULL, 10) > 0)
			profile_period_ns = strtoull(line + 10, NULL, 10);
		for(size_t i = 0; i < sizeof(header) / sizeof(header[0]); i++) {
			size_t size = strlen(header[i].name);
			if(!strncmp(line, header[i].name, size) && line[size] == ':')
				*header[i].value += strtoull(line + size + 1, NULL, 10);
		}
	}
	for(char *line = text; line < text + length; line += strlen(line) + 1) {
		if(!strncmp(line, "changes ", 8)) {
			char *end;
			unsigned long name = strtoul(line + 8, &end, 10);
			if(end == line + 8)
				continue;
			char *at = end;
			uint64_t count = strtoull(at, &end, 10);
			if(end != at)
				tally_add(&profile_row.changes, known_id(name_ids, name_count, name), 0, 0, count);
			continue;
		}
		if(!strncmp(line, "ksample ", 8)) {
			char *at = line + 8, *end;
			uint64_t count = strtoull(at, &end, 10);
			if(end == at || *end != ' ')
				continue;
			at = end + 1;
			size_t name = unknown_id;
			if(*at == '-') {
				name = kernel_only_id;
				at++;
			} else {
				unsigned long index = strtoul(at, &end, 10);
				if(end == at)
					continue;
				name = known_id(name_ids, name_count, index);
				at = end;
			}
			if(*at != ' ')
				continue;
			tally_add(&profile_row.kernel_symbols, name, intern_escaped(at + 1), 0, count);
			continue;
		}
		if(strncmp(line, "sample ", 7) != 0)
			continue;
		char *at = line + 7, *end;
		uint64_t count = strtoull(at, &end, 10);
		if(end == at)
			continue;
		at = end;
		unsigned long name = strtoul(at, &end, 10);
		if(end == at)
			continue;
		at = end;
		while(*at == ' ')
			at++;
		// A sample in no file has a dash for both its file and its offset.
		size_t path = no_file_id;
		uint64_t offset = 0;
		if(*at != '-') {
			unsigned long file = strtoul(at, &end, 10);
			if(end == at)
				continue;
			at = end;
			offset = strtoull(at, &end, 16);
			if(end == at)
				continue;
			path = known_id(file_ids, file_count, file);
		}
		tally_add(&profile_row.samples, known_id(name_ids, name_count, name), path, offset, count);
	}
	free(name_ids);
	free(file_ids);
}

#if HAVE_PERF
// The kernel's ring of samples of each CPU: one page of control and
// PERF_DATA_PAGES of records, a power of two so that a position wraps with
// a mask.
#define PERF_DATA_PAGES 1024
// Added in Linux 3.16; older headers cannot ask for it, and then no record
// says that a process ran a new program.
#ifdef PERF_RECORD_MISC_COMM_EXEC
#define PERF_HAS_COMM_EXEC 1
#else
#define PERF_RECORD_MISC_COMM_EXEC (1 << 13)
#endif

struct perf_ring {
	int fd;
	struct perf_event_mmap_page *page;
};
static struct perf_ring *perf_rings;
static size_t perf_ring_count, perf_page_size;

// A counting event on one CPU, and what it had counted, and for how long
// it was enabled and running, when the pass began.
struct perf_counter {
	int fd;
	size_t name;
	uint64_t before[3];
};
static struct perf_counter *perf_counters;
static size_t perf_counter_count;
// The `nr_tlb_` lines of /proc/vmstat when the pass began.
static struct tally perf_vmstat_before;
static size_t vdso_id;

// An executable mapping of a process, and what the pass knows of a
// process: its parent if it was forked during the pass, its names, and
// whether it ran a new program, which has its own mappings and so does
// not take its parent's.
struct proc_map {
	uint64_t start, end, offset;
	size_t path;
};
struct proc_info {
	uint32_t pid, parent;
	int execd;
	size_t comm, snapshot_comm;
	struct proc_map *maps;
	size_t map_count, map_capacity;
};
static struct proc_info *procs;
static size_t proc_count, proc_capacity;

static struct proc_info *find_proc(uint32_t pid, int create) {
	for(size_t i = 0; i < proc_count; i++) {
		if(procs[i].pid == pid)
			return &procs[i];
	}
	if(!create)
		return NULL;
	if(proc_count == proc_capacity) {
		proc_capacity = proc_capacity ? proc_capacity * 2 : 64;
		procs = realloc(procs, proc_capacity * sizeof(procs[0]));
		if(!procs)
			fail("memory", "out of memory");
	}
	struct proc_info *proc = &procs[proc_count++];
	memset(proc, 0, sizeof(*proc));
	proc->pid = pid;
	proc->comm = proc->snapshot_comm = NO_ID;
	return proc;
}

static void forget_procs(void) {
	for(size_t i = 0; i < proc_count; i++)
		free(procs[i].maps);
	proc_count = 0;
}

static void add_map(struct proc_info *proc, uint64_t start, uint64_t end, uint64_t offset, size_t path) {
	if(proc->map_count == proc->map_capacity) {
		proc->map_capacity = proc->map_capacity ? proc->map_capacity * 2 : 16;
		proc->maps = realloc(proc->maps, proc->map_capacity * sizeof(proc->maps[0]));
		if(!proc->maps)
			fail("memory", "out of memory");
	}
	proc->maps[proc->map_count++] = (struct proc_map){start, end, offset, path};
}

// Whether a mapping's path is one to keep: a file, or the vDSO, which is
// kept so that its addresses are not taken for a parent's mapping.
static int keep_path(const char *path) {
	return (path[0] == '/' && path[1] != '/') || !strcmp(path, "[vdso]");
}

// Notes the processes there are now, with their names and their executable
// mappings, since perf tells of a mapping only when it is made. The servers
// run as the account this runs as, so their maps can be read; those that
// cannot are left out.
static void snapshot_procs(void) {
	DIR *dir = opendir("/proc");
	if(!dir)
		return;
	struct dirent *entry;
	while((entry = readdir(dir))) {
		char *end;
		if(entry->d_name[0] < '0' || entry->d_name[0] > '9')
			continue;
		unsigned long pid = strtoul(entry->d_name, &end, 10);
		if(*end)
			continue;
		struct proc_info *proc = find_proc((uint32_t)pid, 1);
		char path[64], line[8192];
		snprintf(path, sizeof(path), "/proc/%lu/comm", pid);
		FILE *file = fopen(path, "r");
		if(file) {
			if(fgets(line, sizeof(line), file)) {
				line[strcspn(line, "\n")] = 0;
				proc->snapshot_comm = intern(line);
			}
			fclose(file);
		}
		snprintf(path, sizeof(path), "/proc/%lu/maps", pid);
		file = fopen(path, "r");
		while(file && fgets(line, sizeof(line), file)) {
			unsigned long long start, stop, offset;
			char permissions[8];
			int at = 0;
			if(sscanf(line, "%llx-%llx %7s %llx %*s %*s %n", &start, &stop, permissions, &offset, &at) < 4 || at == 0 || !strchr(permissions, 'x'))
				continue;
			line[strcspn(line, "\n")] = 0;
			if(keep_path(line + at))
				add_map(proc, start, stop, offset, intern(line + at));
		}
		if(file)
			fclose(file);
	}
	closedir(dir);
}

static uint32_t get32(const char *at) {
	uint32_t value;
	memcpy(&value, at, sizeof(value));
	return value;
}

static uint64_t get64(const char *at) {
	uint64_t value;
	memcpy(&value, at, sizeof(value));
	return value;
}

// The string at the start of `text`, which has `room` bytes to its end.
static size_t intern_bounded(const char *text, size_t room) {
	size_t length = strnlen(text, room);
	char *copy = malloc(length + 1);
	if(!copy)
		fail("memory", "out of memory");
	memcpy(copy, text, length);
	copy[length] = 0;
	size_t id = intern(copy);
	free(copy);
	return id;
}

// The mapping of a process that holds an address: its own, the newest
// first, then its parent's and the parent's parent's, at most 8 levels up,
// since a process forked without running a program has the layout of the
// one that forked it.
static const struct proc_map *find_map(uint32_t pid, uint64_t ip) {
	for(int level = 0; level <= 8; level++) {
		const struct proc_info *proc = find_proc(pid, 0);
		if(!proc)
			return NULL;
		for(size_t i = proc->map_count; i > 0; i--) {
			if(proc->maps[i - 1].start <= ip && ip < proc->maps[i - 1].end)
				return &proc->maps[i - 1];
		}
		if(proc->execd || proc->parent == 0)
			return NULL;
		pid = proc->parent;
	}
	return NULL;
}

// The name of a process: the one it last took, the one it had when the
// pass began, or its parent's.
static size_t find_name(uint32_t pid) {
	for(int level = 0; level <= 8; level++) {
		const struct proc_info *proc = find_proc(pid, 0);
		if(!proc)
			break;
		if(proc->comm != NO_ID)
			return proc->comm;
		if(proc->snapshot_comm != NO_ID)
			return proc->snapshot_comm;
		if(proc->parent == 0)
			break;
		pid = proc->parent;
	}
	return unknown_id;
}

// Linux's kernel functions, from /proc/kallsyms as root reads it, sorted
// by address: the samples that fell in the kernel are counted by them.
struct kernel_function {
	uint64_t address;
	char *name;
	// The number of its name's string, once a sample has fallen in it.
	size_t id;
};
static struct kernel_function *kernel_functions;
static size_t kernel_function_count;

static int by_function_address(const void *left, const void *right) {
	const struct kernel_function *x = left, *y = right;
	return x->address < y->address ? -1 : x->address > y->address;
}

// Reads the kernel's functions, if root may see their addresses; else
// kernel samples are counted under "?".
static void kallsyms_load(void) {
	FILE *file = fopen("/proc/kallsyms", "r");
	if(!file)
		return;
	char line[512];
	size_t capacity = 0;
	while(fgets(line, sizeof(line), file)) {
		char *end;
		uint64_t address = strtoull(line, &end, 16);
		if(end == line || end[0] != ' ' || !end[1] || end[2] != ' ' || address == 0)
			continue;
		char type = end[1];
		if(type != 't' && type != 'T')
			continue;
		char *name = end + 3;
		name[strcspn(name, " \t\n")] = 0;
		if(kernel_function_count == capacity) {
			capacity = capacity ? capacity * 2 : 65536;
			kernel_functions = realloc(kernel_functions, capacity * sizeof(kernel_functions[0]));
			if(!kernel_functions)
				fail("memory", "out of memory");
		}
		kernel_functions[kernel_function_count++] = (struct kernel_function){address, format("%s", name), NO_ID};
	}
	fclose(file);
	qsort(kernel_functions, kernel_function_count, sizeof(kernel_functions[0]), by_function_address);
}

// The name of the kernel function `ip` fell in, or "?".
static size_t kernel_function_of(uint64_t ip) {
	size_t low = 0, high = kernel_function_count;
	while(low < high) {
		size_t mid = low + (high - low) / 2;
		if(kernel_functions[mid].address <= ip)
			low = mid + 1;
		else
			high = mid;
	}
	if(!low)
		return unknown_id;
	struct kernel_function *function = &kernel_functions[low - 1];
	if(function->id == NO_ID)
		function->id = intern(function->name);
	return function->id;
}

static void perf_sample(uint64_t ip, uint32_t pid, uint16_t misc) {
	if((misc & PERF_RECORD_MISC_CPUMODE_MASK) == PERF_RECORD_MISC_KERNEL || (ip >> 63) != 0) {
		profile_row.kernel++;
		size_t name = pid == 0 ? kernel_only_id : find_name(pid);
		tally_add(&profile_row.kernel_symbols, name, kernel_function_of(ip), 0, 1);
		return;
	}
	profile_row.user++;
	size_t path = no_file_id;
	uint64_t offset = 0;
	const struct proc_map *map = find_map(pid, ip);
	if(map && map->path != vdso_id) {
		path = map->path;
		offset = ip - map->start + map->offset;
	}
	tally_add(&profile_row.samples, find_name(pid), path, offset, 1);
}

// Takes one record of a ring. `body` is what follows the record's header.
// Everything but the samples comes first, since a sample needs the
// mappings and names of the whole pass.
static void perf_record(const struct perf_event_header *header, const char *body, size_t length, int samples) {
	if(header->type == PERF_RECORD_SAMPLE) {
		if(samples && length >= 16)
			perf_sample(get64(body), get32(body + 8), header->misc);
		return;
	}
	if(samples)
		return;
	if(header->type == PERF_RECORD_MMAP && length > 32) {
		size_t path = intern_bounded(body + 32, length - 32);
		if(keep_path(strings[path]))
			add_map(find_proc(get32(body), 1), get64(body + 8), get64(body + 8) + get64(body + 16), get64(body + 24), path);
	} else if(header->type == PERF_RECORD_COMM && length > 8) {
		// A thread's own name is not its process's.
		if(get32(body) == get32(body + 4)) {
			struct proc_info *proc = find_proc(get32(body), 1);
			proc->comm = intern_bounded(body + 8, length - 8);
			if(header->misc & PERF_RECORD_MISC_COMM_EXEC)
				proc->execd = 1;
		}
	} else if(header->type == PERF_RECORD_FORK && length >= 16) {
		// A new thread of a process has the process's own number for its
		// parent.
		if(get32(body) != get32(body + 4))
			find_proc(get32(body), 1)->parent = get32(body + 4);
	} else if(header->type == PERF_RECORD_LOST && length >= 16) {
		profile_row.lost += get64(body + 8);
	}
}

// Reads the records of every ring once. The last sweep gives the space
// back to the kernel.
static void perf_sweep(int samples) {
	// A record is at most 64 KiB; one that wraps the end of the ring is
	// copied here to be read in one piece.
	static uint64_t joined[65536 / sizeof(uint64_t)];
	uint64_t size = (uint64_t)PERF_DATA_PAGES * perf_page_size;
	for(size_t i = 0; i < perf_ring_count; i++) {
		struct perf_event_mmap_page *page = perf_rings[i].page;
		const char *data = (const char *)page + perf_page_size;
		uint64_t head = __atomic_load_n(&page->data_head, __ATOMIC_ACQUIRE);
		uint64_t tail = page->data_tail;
		while(head - tail >= sizeof(struct perf_event_header)) {
			uint64_t at = tail % size;
			struct perf_event_header header;
			memcpy(&header, data + at, sizeof(header));
			if(header.size < sizeof(header) || header.size > head - tail)
				break;
			const char *record = data + at;
			if(at + header.size > size) {
				size_t first = (size_t)(size - at);
				memcpy(joined, data + at, first);
				memcpy((char *)joined + first, data, header.size - first);
				record = (const char *)joined;
			}
			perf_record(&header, record + sizeof(header), header.size - sizeof(header), samples);
			tail += header.size;
		}
		if(samples)
			__atomic_store_n(&page->data_tail, tail, __ATOMIC_RELEASE);
	}
}

static void perf_collect(void) {
	perf_sweep(0);
	perf_sweep(1);
}

// Reads what a counting event has counted, for how long it was enabled and
// for how long it was running; false if the read fails.
static int perf_read_counter(int fd, uint64_t values[3]) {
	return read(fd, values, 3 * sizeof(values[0])) == (ssize_t)(3 * sizeof(values[0]));
}

// Adds each `nr_tlb_` line of Linux's /proc/vmstat to `into`, by the number
// of its name; nothing if the file or such lines are absent.
static void perf_read_vmstat(struct tally *into) {
	FILE *file = fopen("/proc/vmstat", "r");
	if(!file)
		return;
	char line[256];
	while(fgets(line, sizeof(line), file)) {
		char *space = strchr(line, ' ');
		if(strncmp(line, "nr_tlb_", 7) != 0 || !space)
			continue;
		*space = 0;
		tally_add(into, intern(format("linux_%s", line)), 0, 0, strtoull(space + 1, NULL, 10));
	}
	fclose(file);
}

// Takes what the counters count now as what a pass starts from.
static void perf_counters_begin(void) {
	for(size_t i = 0; i < perf_counter_count; i++) {
		if(!perf_read_counter(perf_counters[i].fd, perf_counters[i].before))
			memset(perf_counters[i].before, 0, sizeof(perf_counters[i].before));
	}
	tally_clear(&perf_vmstat_before);
	perf_read_vmstat(&perf_vmstat_before);
}

// Adds what the counters counted since the pass began to the row. A counter
// that the kernel shared with other events was running for only part of the
// time it was enabled, so its count is scaled up to the whole time.
static void perf_counters_end(void) {
	for(size_t i = 0; i < perf_counter_count; i++) {
		uint64_t now[3];
		const uint64_t *before = perf_counters[i].before;
		if(!perf_read_counter(perf_counters[i].fd, now))
			continue;
		uint64_t value = now[0] - before[0], enabled = now[1] - before[1], running = now[2] - before[2];
		if(running == 0)
			value = 0;
		else if(running < enabled)
			value = (uint64_t)((double)value * ((double)enabled / (double)running));
		tally_add(&profile_row.counters, perf_counters[i].name, 0, 0, value);
	}
	struct tally after;
	memset(&after, 0, sizeof(after));
	perf_read_vmstat(&after);
	for(size_t i = 0; i < after.count; i++) {
		uint64_t before = 0;
		for(size_t j = 0; j < perf_vmstat_before.count; j++) {
			if(perf_vmstat_before.entries[j].a == after.entries[i].a)
				before = perf_vmstat_before.entries[j].count;
		}
		tally_add(&profile_row.counters, after.entries[i].a, 0, 0, after.entries[i].count - before);
	}
	free(after.entries);
	free(after.slots);
}

// Starts a pass: notes the processes, drops what the rings hold and starts
// the sampling.
static void perf_begin(void) {
	forget_procs();
	perf_counters_begin();
	snapshot_procs();
	for(size_t i = 0; i < perf_ring_count; i++) {
		struct perf_event_mmap_page *page = perf_rings[i].page;
		__atomic_store_n(&page->data_tail, __atomic_load_n(&page->data_head, __ATOMIC_ACQUIRE), __ATOMIC_RELEASE);
		ioctl(perf_rings[i].fd, PERF_EVENT_IOC_ENABLE, 0);
	}
}

static void perf_stop(void) {
	perf_counters_end();
	for(size_t i = 0; i < perf_ring_count; i++)
		ioctl(perf_rings[i].fd, PERF_EVENT_IOC_DISABLE, 0);
	profile_pending = 1;
}

static void add_cpu(int **cpus, size_t *count, size_t *capacity, int cpu) {
	if(*count == *capacity) {
		*capacity = *capacity ? *capacity * 2 : 16;
		*cpus = realloc(*cpus, *capacity * sizeof((*cpus)[0]));
		if(!*cpus)
			fail("memory", "out of memory");
	}
	(*cpus)[(*count)++] = cpu;
}

// The numbers of the online CPUs, from the ranges of the kernel's list
// ("0-3,5"), or from sysconf if the list cannot be read.
static int *online_cpus(size_t *count) {
	int *cpus = NULL;
	size_t capacity = 0;
	*count = 0;
	FILE *file = fopen("/sys/devices/system/cpu/online", "r");
	char text[1024];
	if(file && fgets(text, sizeof(text), file)) {
		const char *at = text;
		while(*at >= '0' && *at <= '9') {
			char *end;
			long first = strtol(at, &end, 10), last = first;
			if(*end == '-')
				last = strtol(end + 1, &end, 10);
			for(long cpu = first; cpu <= last && cpu < 65536; cpu++)
				add_cpu(&cpus, count, &capacity, (int)cpu);
			at = *end == ',' ? end + 1 : end;
		}
	}
	if(file)
		fclose(file);
	if(*count == 0) {
		long online = sysconf(_SC_NPROCESSORS_ONLN);
		for(long cpu = 0; cpu < (online > 0 ? online : 1); cpu++)
			add_cpu(&cpus, count, &capacity, (int)cpu);
	}
	return cpus;
}

// A hardware event to count: its name, as it is printed, and what
// perf_event_open is given for it.
struct perf_event_choice {
	const char *name;
	uint32_t type;
	uint64_t config;
	// Counts in kernel mode rather than user mode.
	int kernel;
};

#ifdef PERF_CPUID
#define PERF_MAX_EVENTS 10

// The events that this processor can count, up to PERF_MAX_EVENTS of them,
// as the processor's maker and model make them known; none for a processor
// not known. Returns how many.
static size_t perf_choose_events(struct perf_event_choice *events) {
	unsigned eax, ebx, ecx, edx;
	if(!__get_cpuid(0, &eax, &ebx, &ecx, &edx))
		return 0;
	char vendor[13];
	memcpy(vendor, &ebx, 4);
	memcpy(vendor + 4, &edx, 4);
	memcpy(vendor + 8, &ecx, 4);
	vendor[12] = 0;
	unsigned highest = eax;
	if(!__get_cpuid(1, &eax, &ebx, &ecx, &edx))
		return 0;
	unsigned family = (eax >> 8) & 0xf, model = (eax >> 4) & 0xf;
	if(family == 0xf)
		family += (eax >> 20) & 0xff;
	if(((eax >> 8) & 0xf) == 6 || ((eax >> 8) & 0xf) == 0xf)
		model += ((eax >> 16) & 0xf) << 4;
	size_t count = 0;
	if(!strcmp(vendor, "GenuineIntel")) {
		if(highest < 0xa)
			return 0;
		__cpuid_count(0xa, 0, eax, ebx, ecx, edx);
		if((eax & 0xff) < 2)
			return 0;
		unsigned general = (eax >> 8) & 0xff;
		events[count++] = (struct perf_event_choice){"instructions", PERF_TYPE_HARDWARE, PERF_COUNT_HW_INSTRUCTIONS, 0};
		events[count++] = (struct perf_event_choice){"cycles", PERF_TYPE_HARDWARE, PERF_COUNT_HW_CPU_CYCLES, 0};
		if(family != 6 || (model != 0x4E && model != 0x5E && model != 0x8E && model != 0x9E && model != 0xA5 && model != 0xA6))
			return count;
		// The same events as the Radix kernel's pmu.rs counts, in the same
		// order and modes.
		static const struct {
			const char *name;
			unsigned event, umask;
			int kernel;
		} raw[] = {{"kernel_instructions", 0xC0, 0x00, 1}, {"kernel_cycles", 0x3C, 0x00, 1},
		           {"kernel_l2_misses", 0x24, 0x3F, 1},    {"kernel_llc_misses", 0x2E, 0x41, 1},
		           {"kernel_dtlb_load_walks", 0x08, 0x0E, 1}, {"l2_misses", 0x24, 0x3F, 0},
		           {"llc_misses", 0x2E, 0x41, 0},          {"dtlb_load_walks", 0x08, 0x0E, 0}};
		for(size_t i = 0; i < sizeof(raw) / sizeof(raw[0]) && i < general && i < 8; i++)
			events[count++] = (struct perf_event_choice){raw[i].name, PERF_TYPE_RAW, raw[i].event | ((uint64_t)raw[i].umask << 8), raw[i].kernel};
	} else if(!strcmp(vendor, "AuthenticAMD") || !strcmp(vendor, "HygonGenuine")) {
		if(family < 0x17 || family > 0x1A)
			return 0;
		unsigned extended = __get_cpuid(0x80000001, &eax, &ebx, &ecx, &edx) ? (ecx >> 23) & 1 : 0;
		static const struct {
			const char *name;
			unsigned event, umask;
		} raw[] = {{"instructions", 0xC0, 0x00}, {"cycles", 0x76, 0x00}, {"dtlb_walks", 0x45, 0xF0},
		           {"itlb_walks", 0x85, 0x0F},   {"l2_misses", 0x64, 0x09}};
		for(size_t i = 0; i < sizeof(raw) / sizeof(raw[0]) && i < (extended ? 6u : 4u); i++)
			events[count++] = (struct perf_event_choice){
				raw[i].name, PERF_TYPE_RAW,
				(raw[i].event & 0xff) | ((uint64_t)raw[i].umask << 8) | ((uint64_t)(raw[i].event >> 8) << 32), 0};
	}
	return count;
}
#endif

// Opens the counting events of this processor on each of `cpus`, for all
// processes, counting user mode, or kernel mode for a kernel_ event. An event that a CPU refuses is left
// out for that CPU, and one that none accepts is never printed; counting
// is a help to the profile, not part of it.
static void perf_open_counters(const int *cpus, size_t cpu_count) {
#ifdef PERF_CPUID
	struct perf_event_choice events[PERF_MAX_EVENTS];
	size_t event_count = perf_choose_events(events);
	for(size_t e = 0; e < event_count; e++) {
		for(size_t i = 0; i < cpu_count; i++) {
			struct perf_event_attr attr;
			memset(&attr, 0, sizeof(attr));
			attr.type = events[e].type;
			attr.size = sizeof(attr);
			attr.config = events[e].config;
			attr.read_format = PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING;
			attr.exclude_kernel = !events[e].kernel;
			attr.exclude_user = events[e].kernel;
			attr.exclude_hv = 1;
			int fd = (int)syscall(SYS_perf_event_open, &attr, -1, cpus[i], -1, PERF_FLAG_FD_CLOEXEC);
			if(fd < 0)
				continue;
			struct perf_counter *grown = realloc(perf_counters, (perf_counter_count + 1) * sizeof(perf_counters[0]));
			if(!grown) {
				close(fd);
				continue;
			}
			perf_counters = grown;
			perf_counters[perf_counter_count++] = (struct perf_counter){fd, intern(events[e].name), {0, 0, 0}};
		}
	}
#else
	(void)cpus;
	(void)cpu_count;
#endif
}

// Opens a software clock event on each online CPU for all processes, with
// a ring each, stopped until a pass begins. Null if all went well, else why
// not; what was opened is closed. perf_event_paranoid and the limit on
// locked memory refuse an account other than root, so this runs before the
// program becomes `bench`; the files and rings stay usable after that.
static char *perf_open_all(void) {
	perf_page_size = (size_t)sysconf(_SC_PAGESIZE);
	kallsyms_load();
	size_t count;
	int *cpus = online_cpus(&count);
	char *why = NULL;
	for(size_t i = 0; i < count && !why; i++) {
		struct perf_event_attr attr;
		memset(&attr, 0, sizeof(attr));
		attr.type = PERF_TYPE_SOFTWARE;
		attr.size = sizeof(attr);
		attr.config = PERF_COUNT_SW_CPU_CLOCK;
		attr.sample_period = PROFILE_PERIOD_NS;
		attr.sample_type = PERF_SAMPLE_IP | PERF_SAMPLE_TID;
		attr.disabled = 1;
		attr.mmap = 1;
		attr.comm = 1;
		attr.task = 1;
		attr.exclude_hv = 1;
#ifdef PERF_HAS_COMM_EXEC
		attr.comm_exec = 1;
#endif
		int fd = (int)syscall(SYS_perf_event_open, &attr, -1, cpus[i], -1, PERF_FLAG_FD_CLOEXEC);
		if(fd < 0) {
			why = format("perf_event_open for CPU %d failed: %s", cpus[i], strerror(errno));
			break;
		}
		void *page = mmap(NULL, (1 + (size_t)PERF_DATA_PAGES) * perf_page_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
		if(page == MAP_FAILED) {
			why = format("cannot map the perf ring of CPU %d: %s", cpus[i], strerror(errno));
			close(fd);
			break;
		}
		perf_rings = realloc(perf_rings, (perf_ring_count + 1) * sizeof(perf_rings[0]));
		if(!perf_rings)
			fail("memory", "out of memory");
		perf_rings[perf_ring_count++] = (struct perf_ring){fd, page};
	}
	if(!why)
		perf_open_counters(cpus, count);
	free(cpus);
	if(why) {
		for(size_t i = 0; i < perf_ring_count; i++) {
			munmap(perf_rings[i].page, (1 + (size_t)PERF_DATA_PAGES) * perf_page_size);
			close(perf_rings[i].fd);
		}
		perf_ring_count = 0;
	}
	return why;
}
#endif

// A file's loadable segments and function symbols, as far as they can be
// read: 64-bit little-endian ELF only, which is all the servers are.
struct elf_header {
	uint8_t ident[16];
	uint16_t type, machine;
	uint32_t version;
	uint64_t entry, program_offset, section_offset;
	uint32_t flags;
	uint16_t header_size, program_size, program_count, section_size, section_count, names_section;
};
struct elf_program {
	uint32_t type, flags;
	uint64_t offset, vaddr, paddr, file_size, memory_size, align;
};
struct elf_section {
	uint32_t name, type;
	uint64_t flags, address, offset, size;
	uint32_t link, info;
	uint64_t align, entry_size;
};
struct elf_sym {
	uint32_t name;
	uint8_t info, other;
	uint16_t section;
	uint64_t value, size;
};
_Static_assert(sizeof(struct elf_header) == 64 && sizeof(struct elf_program) == 56 && sizeof(struct elf_section) == 64 &&
               sizeof(struct elf_sym) == 24, "the ELF structures have the sizes of the file format");

struct elf_symbol {
	uint64_t address, size;
	const char *name;
	int rank;
};
struct elf_file {
	size_t path;
	struct elf_program *segments;
	size_t segment_count;
	struct elf_symbol *symbols;
	size_t symbol_count;
};
static struct elf_file *elf_files;
static size_t elf_file_count;

// Reads `size` bytes at `offset` into a new buffer with a zero after them,
// null if the file is too short, so that a damaged table cannot make the
// program read beyond the file.
static void *read_part(int fd, uint64_t file_size, uint64_t offset, uint64_t size) {
	if(offset > file_size || size > file_size - offset)
		return NULL;
	char *bytes = malloc((size_t)size + 1);
	if(!bytes)
		fail("memory", "out of memory");
	size_t done = 0;
	while(done < size) {
		ssize_t count = pread(fd, bytes + done, (size_t)size - done, (off_t)(offset + done));
		if(count < 0 && errno == EINTR)
			continue;
		if(count <= 0) {
			free(bytes);
			return NULL;
		}
		done += (size_t)count;
	}
	bytes[size] = 0;
	return bytes;
}

static int by_address(const void *left, const void *right) {
	const struct elf_symbol *x = left, *y = right;
	if(x->address != y->address)
		return x->address < y->address ? -1 : 1;
	if(x->rank != y->rank)
		return x->rank < y->rank ? -1 : 1;
	if(x->size != y->size)
		return x->size < y->size ? -1 : 1;
	return strcmp(x->name, y->name);
}

// Reads the segments and the functions of an ELF file, from its symbol
// table if it has one, else from its dynamic symbols; the file is left
// with none of either if it cannot be read. Of symbols at one address,
// the global one is the one that sorts last, which is the one found.
static void elf_load(struct elf_file *file, const char *path) {
	int fd = open(path, O_RDONLY);
	struct stat status;
	if(fd < 0)
		return;
	struct elf_header *header = NULL;
	struct elf_section *sections = NULL;
	if(fstat(fd, &status) == 0 && status.st_size > 0)
		header = read_part(fd, (uint64_t)status.st_size, 0, sizeof(*header));
	if(header && (memcmp(header->ident, "\177ELF", 4) != 0 || header->ident[4] != 2 || header->ident[5] != 1 ||
	              header->program_size != sizeof(struct elf_program) || header->section_size != sizeof(struct elf_section))) {
		free(header);
		header = NULL;
	}
	if(!header) {
		close(fd);
		return;
	}
	uint64_t file_size = (uint64_t)status.st_size;
	struct elf_program *programs = read_part(fd, file_size, header->program_offset, (uint64_t)header->program_count * sizeof(*programs));
	for(size_t i = 0; programs && i < header->program_count; i++) {
		if(programs[i].type == 1) {
			file->segments = realloc(file->segments, (file->segment_count + 1) * sizeof(file->segments[0]));
			if(!file->segments)
				fail("memory", "out of memory");
			file->segments[file->segment_count++] = programs[i];
		}
	}
	free(programs);
	if(header->section_count)
		sections = read_part(fd, file_size, header->section_offset, (uint64_t)header->section_count * sizeof(*sections));
	// SHT_SYMTAB is 2 and SHT_DYNSYM 11.
	const struct elf_section *table = NULL;
	for(unsigned want = 2; want <= 11 && !table; want += 9) {
		for(size_t i = 0; sections && i < header->section_count; i++) {
			if(sections[i].type == want && sections[i].link < header->section_count) {
				table = &sections[i];
				break;
			}
		}
	}
	struct elf_sym *syms = NULL;
	char *names = NULL;
	if(table) {
		syms = read_part(fd, file_size, table->offset, table->size);
		names = read_part(fd, file_size, sections[table->link].offset, sections[table->link].size);
	}
	if(syms && names) {
		size_t count = (size_t)(table->size / sizeof(*syms));
		size_t names_size = (size_t)sections[table->link].size;
		file->symbols = malloc((count + 1) * sizeof(file->symbols[0]));
		if(!file->symbols)
			fail("memory", "out of memory");
		for(size_t i = 0; i < count; i++) {
			// STT_FUNC is 2 and STT_GNU_IFUNC 10; STB_LOCAL is 0, STB_WEAK 2.
			unsigned type = syms[i].info & 0xf, bind = syms[i].info >> 4;
			if((type != 2 && type != 10) || syms[i].section == 0 || syms[i].value == 0 || syms[i].name >= names_size)
				continue;
			file->symbols[file->symbol_count++] =
				(struct elf_symbol){syms[i].value, syms[i].size, names + syms[i].name, bind == 0 ? 0 : bind == 2 ? 1 : 2};
		}
		qsort(file->symbols, file->symbol_count, sizeof(file->symbols[0]), by_address);
		// The names stay in use, so their buffer is kept.
		names = NULL;
	}
	free(names);
	free(syms);
	free(sections);
	free(header);
	close(fd);
}

// The function at an offset in a file, or "?": the offset becomes an
// address through the segment that holds it, and the symbol is the last
// that starts at or before the address, if the address is within its size
// (or it has none).
static const char *symbol_of(size_t path, uint64_t offset) {
	struct elf_file *file = NULL;
	for(size_t i = 0; i < elf_file_count && !file; i++) {
		if(elf_files[i].path == path)
			file = &elf_files[i];
	}
	if(!file) {
		elf_files = realloc(elf_files, (elf_file_count + 1) * sizeof(elf_files[0]));
		if(!elf_files)
			fail("memory", "out of memory");
		file = &elf_files[elf_file_count++];
		memset(file, 0, sizeof(*file));
		file->path = path;
		if(strings[path][0] == '/')
			elf_load(file, strings[path]);
	}
	for(size_t i = 0; i < file->segment_count; i++) {
		const struct elf_program *segment = &file->segments[i];
		if(segment->offset <= offset && offset - segment->offset < segment->file_size) {
			uint64_t address = offset - segment->offset + segment->vaddr;
			size_t low = 0, high = file->symbol_count;
			while(low < high) {
				size_t middle = low + (high - low) / 2;
				if(file->symbols[middle].address <= address)
					low = middle + 1;
				else
					high = middle;
			}
			if(low == 0)
				return "?";
			const struct elf_symbol *symbol = &file->symbols[low - 1];
			return symbol->size == 0 || address < symbol->address + symbol->size ? symbol->name : "?";
		}
	}
	return "?";
}

// Prints a name, path or symbol as one word: a space or a control
// character becomes an underscore, so that the fields stay apart.
static void print_word(const char *text) {
	if(!*text)
		text = "?";
	for(; *text; text++)
		putchar((unsigned char)*text <= ' ' ? '_' : *text);
}

static void print_share(uint64_t samples, uint64_t user) {
	printf(" samples=%llu share=%.2f\n", (unsigned long long)samples, user ? 100.0 * (double)samples / (double)user : 0.0);
}

// Takes the samples of a pass that has ended into the row.
static void profile_collect(void) {
	if(!profile_pending)
		return;
	profile_pending = 0;
	if(profile_radix) {
		radix_collect(profile_text);
		free(profile_text);
		profile_text = NULL;
		return;
	}
#if HAVE_PERF
	perf_collect();
#endif
}

// Prints what the passes of a row sampled, as BENCHPROF lines, and starts
// the next row with nothing.
static void profile_print(const char *row) {
	if(!profile_on)
		return;
	profile_collect();
	struct tally objects, symbols;
	memset(&objects, 0, sizeof(objects));
	memset(&symbols, 0, sizeof(symbols));
	for(size_t i = 0; i < profile_row.samples.count; i++) {
		const struct tally_entry *entry = &profile_row.samples.entries[i];
		const char *path = strings[entry->b];
		const char *slash = strrchr(path, '/');
		size_t object = slash && path[0] == '/' ? intern(slash + 1) : entry->b;
		tally_add(&objects, entry->a, object, 0, entry->count);
		// The symbol's name is kept for the rest of the run, so its address
		// stands for it.
		tally_add(&symbols, entry->a, object, (uint64_t)(uintptr_t)symbol_of(entry->b, entry->c), entry->count);
	}
	printf("BENCHPROF group=%s period_ns=%llu passes=%d user=%llu others=%llu kernel=%llu lost=%llu left_out=%llu\n", row,
	       (unsigned long long)profile_period_ns, profile_row.passes, (unsigned long long)profile_row.user,
	       (unsigned long long)profile_row.others, (unsigned long long)profile_row.kernel, (unsigned long long)profile_row.lost,
	       (unsigned long long)profile_row.left_out);
	for(size_t i = 0; i < profile_row.counters.count; i++) {
		printf("BENCHPROF group=%s counter name=", row);
		print_word(strings[profile_row.counters.entries[i].a]);
		printf(" value=%llu\n", (unsigned long long)profile_row.counters.entries[i].count);
	}
	struct tally_entry *sorted = tally_sorted(&profile_row.changes);
	for(size_t i = 0; i < profile_row.changes.count && sorted[i].count > 0; i++) {
		printf("BENCHPROF group=%s changes process=", row);
		print_word(strings[sorted[i].a]);
		printf(" count=%llu\n", (unsigned long long)sorted[i].count);
	}
	free(sorted);
	sorted = tally_sorted(&objects);
	for(size_t i = 0; i < objects.count; i++) {
		printf("BENCHPROF group=%s object process=", row);
		print_word(strings[sorted[i].a]);
		printf(" object=");
		print_word(strings[sorted[i].b]);
		print_share(sorted[i].count, profile_row.user);
	}
	free(sorted);
	sorted = tally_sorted(&symbols);
	for(size_t i = 0; i < symbols.count && i < PROFILE_SYMBOLS; i++) {
		printf("BENCHPROF group=%s symbol process=", row);
		print_word(strings[sorted[i].a]);
		printf(" object=");
		print_word(strings[sorted[i].b]);
		printf(" symbol=");
		print_word((const char *)(uintptr_t)sorted[i].c);
		print_share(sorted[i].count, profile_row.user);
	}
	free(sorted);
	sorted = tally_sorted(&profile_row.kernel_symbols);
	for(size_t i = 0; i < profile_row.kernel_symbols.count && i < PROFILE_KERNEL_SYMBOLS; i++) {
		printf("BENCHPROF group=%s ksymbol process=", row);
		print_word(strings[sorted[i].a]);
		printf(" symbol=");
		print_word(strings[sorted[i].b]);
		print_share(sorted[i].count, profile_row.kernel);
	}
	free(sorted);
	if(profile_raw) {
		sorted = tally_sorted(&profile_row.samples);
		for(size_t i = 0; i < profile_row.samples.count; i++) {
			printf("BENCHPROF group=%s raw process=", row);
			print_word(strings[sorted[i].a]);
			printf(" path=");
			print_word(strings[sorted[i].b]);
			printf(" offset=0x%llx samples=%llu\n", (unsigned long long)sorted[i].c, (unsigned long long)sorted[i].count);
		}
		free(sorted);
	}
	fflush(stdout);
	free(objects.entries);
	free(objects.slots);
	free(symbols.entries);
	free(symbols.slots);
	tally_clear(&profile_row.samples);
	tally_clear(&profile_row.counters);
	tally_clear(&profile_row.changes);
	tally_clear(&profile_row.kernel_symbols);
	profile_row.passes = 0;
	profile_row.user = profile_row.others = profile_row.kernel = profile_row.lost = profile_row.left_out = 0;
}

// Starts sampling if SERVERBENCH_PROFILE asks for it and the system can: on
// Radix by reading /processes/profile, whose first open starts the kernel's
// sampling, and on Linux by opening perf events, which needs root and so
// comes before make_work gives it up. A system that cannot says so once.
static void profile_open(void) {
	const char *setting = getenv("SERVERBENCH_PROFILE");
	if(!setting || !*setting)
		return;
	no_file_id = intern("[no file]");
	unknown_id = intern("?");
	kernel_only_id = intern("[kernel]");
	profile_raw = !strcmp(setting, "raw");
	const char *why = NULL;
	if(access("/processes/profile", F_OK) == 0) {
		char *first = read_all("/processes/profile");
		if(first)
			profile_radix = 1;
		else
			why = format("cannot read /processes/profile: %s", strerror(errno));
		free(first);
	} else {
#if HAVE_PERF
		vdso_id = intern("[vdso]");
		why = perf_open_all();
#else
		why = "this system has no perf_event_open";
#endif
	}
	if(why) {
		printf("serverbench: no profile: %s\n", why);
		fflush(stdout);
		return;
	}
	profile_on = 1;
}

// Right before a measured pass starts. On Radix reading the file starts the
// pass's samples afresh; on Linux the processes are noted and sampling is
// switched on.
static void profile_begin(void) {
	if(!profile_on)
		return;
	profile_collect();
	if(profile_radix)
		free(read_all("/processes/profile"));
#if HAVE_PERF
	else
		perf_begin();
#endif
}

// Right after a measured pass ends. Radix's text is only read here and
// Linux's rings are only stopped, and the rest is left to the next
// begin or the print.
static void profile_end(void) {
	if(!profile_on)
		return;
	profile_row.passes++;
	if(profile_radix) {
		free(profile_text);
		profile_text = read_all("/processes/profile");
		profile_pending = profile_text != NULL;
		return;
	}
#if HAVE_PERF
	perf_stop();
#endif
}

// The path of a program of a bundle.
static char *program(const char *bundle, const char *name) {
	return format("%s/%s/bin/%s", applications, bundle, name);
}

static int remove_entry(const char *path, const struct stat *status, int type, struct FTW *position) {
	(void)status;
	(void)type;
	(void)position;
	// A path that cannot be removed stays for the person to see; the walk
	// goes on to the rest.
	remove(path);
	return 0;
}

static void make_dir(const char *path) {
	if(mkdir(path, 0700) != 0 && errno != EEXIST)
		fail("work folder", "cannot make %s: %s", path, strerror(errno));
}

// Writes a file in the work folder.
static void write_file(const char *name, const char *bytes, size_t length) {
	char *path = format("%s/%s", work, name);
	int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
	if(fd < 0)
		fail("work folder", "cannot write %s: %s", path, strerror(errno));
	if(write(fd, bytes, length) != (ssize_t)length)
		fail("work folder", "cannot write %s: %s", path, strerror(errno));
	close(fd);
}

// Writes a file of the work folder from a text in which `$WORK` stands for
// the work folder's path.
static void write_template(const char *name, const char *text) {
	size_t places = 0;
	for(const char *at = strstr(text, "$WORK"); at; at = strstr(at + 5, "$WORK"))
		places++;
	size_t copied = 0;
	char *result = malloc(strlen(text) + places * strlen(work) + 1);
	if(!result)
		fail("memory", "out of memory");
	for(const char *at = text; *at;) {
		if(!strncmp(at, "$WORK", 5)) {
			strcpy(result + copied, work);
			copied += strlen(work);
			at += 5;
		} else {
			result[copied++] = *at++;
		}
	}
	write_file(name, result, copied);
	free(result);
}

// Stops every server that is still running, then removes the work folder.
static void stop_server(struct child *child);
static void cleanup(void) {
	for(size_t i = 0; i < sizeof(children) / sizeof(children[0]); i++)
		stop_server(&children[i]);
	if(work_made)
		nftw(work, remove_entry, 16, FTW_DEPTH | FTW_PHYS);
}

// Starts a program with its output in <log>, or in the null device when
// `log` is null. Each gets a process group of its own, so that a server
// that will not stop can be killed with the processes it made.
static pid_t spawn(const char *log, char *const argv[]) {
	pid_t pid = fork();
	if(pid < 0)
		fail(argv[0], "cannot fork: %s", strerror(errno));
	if(pid > 0)
		return pid;
	setpgid(0, 0);
	// The parent ignores SIGPIPE; a server expects the default.
	signal(SIGPIPE, SIG_DFL);
	int null = open("/dev/null", O_RDWR);
	int output = null;
	if(log)
		output = open(log, O_WRONLY | O_CREAT | O_APPEND, 0600);
	if(null < 0 || output < 0 || dup2(null, 0) < 0 || dup2(output, 1) < 0 || dup2(output, 2) < 0)
		_exit(126);
	execve(argv[0], argv, environment);
	fprintf(stderr, "cannot run %s: %s\n", argv[0], strerror(errno));
	_exit(127);
}

static void remember_log(const char *name) {
	for(size_t i = 0; i < log_count; i++) {
		if(!strcmp(logs[i], name))
			return;
	}
	logs[log_count++] = name;
}

// Runs a program to its end with its output in <work>/<name>.log, which
// is empty when it starts; any other end than exit status 0 is a failure.
static void run_tool(const char *name, char *const argv[]) {
	char *log = format("%s/%s.log", work, name);
	unlink(log);
	remember_log(name);
	pid_t pid = spawn(log, argv);
	int status;
	struct rusage usage;
	while(wait4(pid, &status, 0, &usage) < 0) {
		if(errno != EINTR)
			fail(name, "cannot wait for it: %s", strerror(errno));
	}
	tool_user_ns = timeval_ns(usage.ru_utime);
	tool_system_ns = timeval_ns(usage.ru_stime);
	if(!WIFEXITED(status))
		fail(name, "ended by signal %d", WTERMSIG(status));
	if(WEXITSTATUS(status) != 0)
		fail(name, "exited with status %d", WEXITSTATUS(status));
}

// Starts a server that runs until it is stopped. `stop_signal` is what
// asks it to end: SIGTERM, or SIGINT for PostgreSQL's fast shutdown.
static struct child *start_server(const char *name, int stop_signal, char *const argv[]) {
	struct child *child = children;
	while(child->pid != 0)
		child++;
	remember_log(name);
	child->name = name;
	child->stop_signal = stop_signal;
	child->pid = spawn(format("%s/%s.log", work, name), argv);
	return child;
}

// Asks a server to end, kills it and what it started if it has not ended
// after STOP_SECONDS, and reaps it. Its exit status says nothing: the
// rows have been measured by now.
static void stop_server(struct child *child) {
	if(child->pid == 0)
		return;
	kill(child->pid, child->stop_signal);
	struct timespec pause = {.tv_sec = 0, .tv_nsec = 20 * 1000 * 1000};
	for(int waited = 0; waited < STOP_SECONDS * 50; waited++) {
		if(waitpid(child->pid, NULL, WNOHANG) == child->pid) {
			child->pid = 0;
			return;
		}
		nanosleep(&pause, NULL);
	}
	kill(-child->pid, SIGKILL);
	while(waitpid(child->pid, NULL, 0) < 0 && errno == EINTR) {}
	child->pid = 0;
}

static struct sockaddr_in loopback(int port) {
	struct sockaddr_in address;
	memset(&address, 0, sizeof(address));
	address.sin_family = AF_INET;
	address.sin_port = htons((uint16_t)port);
	address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
	return address;
}

// Whether something accepts a connection at `address`.
static int accepts(const struct sockaddr *address, socklen_t length) {
	int fd = socket(address->sa_family, SOCK_STREAM, 0);
	if(fd < 0)
		return 0;
	int accepted = connect(fd, address, length) == 0;
	close(fd);
	return accepted;
}

static int tcp_ready(int port) {
	struct sockaddr_in address = loopback(port);
	return accepts((struct sockaddr *)&address, sizeof(address));
}

static int nginx_ready(void) {
	return tcp_ready(NGINX_PORT);
}

static int valkey_ready(void) {
	return tcp_ready(VALKEY_PORT);
}

static int fpm_ready(void) {
	struct sockaddr_un address;
	memset(&address, 0, sizeof(address));
	address.sun_family = AF_UNIX;
	snprintf(address.sun_path, sizeof(address.sun_path), "%s/php-fpm.sock", work);
	return accepts((struct sockaddr *)&address, sizeof(address));
}

static int postgres_ready(void) {
	char *argv[] = {program("org.postgresql.postgresql", "pg_isready"), "-h", work, "-U", "bench", "-d", "postgres", NULL};
	pid_t pid = spawn(NULL, argv);
	int status;
	while(waitpid(pid, &status, 0) < 0 && errno == EINTR) {}
	return WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

// Waits until `ready` says the server is, polling every 50 ms; a server
// that ends first, or is not ready after READY_SECONDS, is a failure.
static void wait_ready(struct child *child, int (*ready)(void)) {
	struct timespec pause = {.tv_sec = 0, .tv_nsec = 50 * 1000 * 1000};
	for(int waited = 0; waited < READY_SECONDS * 20; waited++) {
		int status;
		if(waitpid(child->pid, &status, WNOHANG) == child->pid) {
			child->pid = 0;
			fail(child->name, "it ended at start (status %d)", WIFEXITED(status) ? WEXITSTATUS(status) : -WTERMSIG(status));
		}
		if(ready())
			return;
		nanosleep(&pause, NULL);
	}
	fail(child->name, "it is not ready after %d s", READY_SECONDS);
}

// The FastCGI row's page: about 2.7 KiB of JSON. It sets a Content-Length,
// since nginx would otherwise send the answer chunked and the client
// reads lengths.
static const char page_php[] =
	"<?php\n"
	"$records = [];\n"
	"for ($i = 0; $i < 50; $i++) {\n"
	"    $records[] = ['id' => $i, 'name' => 'item-' . $i, 'price' => $i * 1.25,\n"
	"                  'tags' => ['alpha', 'beta', 'gamma']];\n"
	"}\n"
	"$body = json_encode($records);\n"
	"header('Content-Type: application/json');\n"
	"header('Content-Length: ' . strlen($body));\n"
	"echo $body;\n";

// The files nginx serves: the two static files hold the letters A to Z
// over and over, and the client checks their length, not their bytes.
static void write_web_files(void) {
	static char pattern[LARGE_SIZE];
	for(size_t i = 0; i < sizeof(pattern); i++)
		pattern[i] = (char)('A' + i % 26);
	make_dir(format("%s/www", work));
	write_file("www/small.html", pattern, SMALL_SIZE);
	write_file("www/large.bin", pattern, LARGE_SIZE);
	write_file("www/page.php", page_php, sizeof(page_php) - 1);
}

static const char nginx_conf[] =
	"daemon off;\n"
	"master_process on;\n"
	"worker_processes 1;\n"
	"pid $WORK/nginx/nginx.pid;\n"
	"events { worker_connections 256; }\n"
	"http {\n"
	"\taccess_log off;\n"
	"\tsendfile off;\n"
	"\tkeepalive_requests 1000000;\n"
	"\tkeepalive_timeout 65;\n"
	"\tclient_body_temp_path $WORK/nginx/client_body;\n"
	"\tproxy_temp_path $WORK/nginx/proxy;\n"
	"\tfastcgi_temp_path $WORK/nginx/fastcgi;\n"
	"\tuwsgi_temp_path $WORK/nginx/uwsgi;\n"
	"\tscgi_temp_path $WORK/nginx/scgi;\n"
	"\ttypes { text/html html; application/octet-stream bin; }\n"
	"\tserver {\n"
	"\t\tlisten 127.0.0.1:18080;\n"
	"\t\troot $WORK/www;\n"
	"\t\tlocation = /page.php {\n"
	"\t\t\tfastcgi_pass unix:$WORK/php-fpm.sock;\n"
	"\t\t\tfastcgi_param SCRIPT_FILENAME $WORK/www/page.php;\n"
	"\t\t\tfastcgi_param REQUEST_METHOD $request_method;\n"
	"\t\t\tfastcgi_param QUERY_STRING $query_string;\n"
	"\t\t\tfastcgi_param SERVER_PROTOCOL $server_protocol;\n"
	"\t\t}\n"
	"\t}\n"
	"}\n";

static struct child *start_nginx(void) {
	write_web_files();
	make_dir(format("%s/nginx", work));
	write_template("nginx/nginx.conf", nginx_conf);
	char *argv[] = {program("org.nginx.nginx", "nginx"),
	                "-p", format("%s/nginx/", work),
	                "-c", format("%s/nginx/nginx.conf", work),
	                "-e", format("%s/nginx.log", work), NULL};
	struct child *nginx = start_server("nginx", SIGTERM, argv);
	wait_ready(nginx, nginx_ready);
	return nginx;
}

static struct child *start_fpm(void) {
	write_template("php-fpm.conf",
	               "[global]\n"
	               "error_log = $WORK/php-fpm.log\n"
	               "daemonize = no\n"
	               "[www]\n"
	               "listen = $WORK/php-fpm.sock\n"
	               "pm = static\n"
	               "pm.max_children = 4\n");
	char *argv[] = {program("net.php.php", "php-fpm"), "-n", "-y", format("%s/php-fpm.conf", work), "-F", NULL};
	struct child *fpm = start_server("php-fpm", SIGTERM, argv);
	wait_ready(fpm, fpm_ready);
	return fpm;
}

// The HTTP row being measured, which a failure names.
static const char *http_row_now = "http";

// One HTTP request in flight, or none.
struct connection {
	int fd;
	int connecting;
	int in_flight;
	// The response's header as far as it has arrived, with a terminator.
	char head[4096 + 1];
	size_t head_length;
	int head_done;
	uint64_t body_left;
};

static void send_request(struct connection *connection, const char *request, size_t length) {
	if(write(connection->fd, request, length) != (ssize_t)length)
		fail(http_row_now, "cannot send a request: %s", strerror(errno));
}

// Starts a request on a connection, opening the connection first if it has
// none (the connect row's, and the first of each other row's).
static void issue(struct connection *connection, const char *request, size_t length) {
	connection->head_length = 0;
	connection->head_done = 0;
	connection->in_flight = 1;
	if(connection->fd >= 0) {
		send_request(connection, request, length);
		return;
	}
	connection->fd = socket(AF_INET, SOCK_STREAM, 0);
	if(connection->fd < 0 || fcntl(connection->fd, F_SETFL, O_NONBLOCK) != 0)
		fail(http_row_now, "cannot make a socket: %s", strerror(errno));
	struct sockaddr_in address = loopback(NGINX_PORT);
	if(connect(connection->fd, (struct sockaddr *)&address, sizeof(address)) == 0)
		send_request(connection, request, length);
	else if(errno == EINPROGRESS)
		connection->connecting = 1;
	else
		fail(http_row_now, "cannot connect to nginx: %s", strerror(errno));
}

// Reads the status and the length of the body from a response's header,
// which ends at `end`, and counts the body bytes that arrived with it.
static void read_head(struct connection *connection, const char *end, uint64_t expected) {
	if(strncmp(connection->head, "HTTP/1.1 200 ", 13) != 0) {
		char *line = strchr(connection->head, '\r');
		fail(http_row_now, "the answer is \"%.*s\", not 200", (int)(line ? line - connection->head : 40), connection->head);
	}
	int found = 0;
	uint64_t length = 0;
	for(const char *line = strstr(connection->head, "\r\n"); line && line < end; line = strstr(line + 2, "\r\n")) {
		if(strncasecmp(line + 2, "content-length:", 15) == 0) {
			length = strtoull(line + 17, NULL, 10);
			found = 1;
		}
	}
	if(!found)
		fail(http_row_now, "the answer has no Content-Length");
	if(expected != 0 && length != expected)
		fail(http_row_now, "the body is %llu bytes, not %llu", (unsigned long long)length, (unsigned long long)expected);
	size_t arrived = connection->head_length - (size_t)(end + 4 - connection->head);
	if(arrived > length)
		fail(http_row_now, "the body is longer than its Content-Length");
	connection->head_done = 1;
	connection->body_left = length - arrived;
}

// Takes what has arrived on a connection; returns 1 when its response is
// complete. A response with `keep_alive` is complete with its last body
// byte, and one without ends with the server's close.
static int read_response(struct connection *connection, int keep_alive, uint64_t expected) {
	static char body[65536];
	ssize_t count;
	if(connection->head_done)
		count = read(connection->fd, body, sizeof(body));
	else
		count = read(connection->fd, connection->head + connection->head_length, sizeof(connection->head) - 1 - connection->head_length);
	if(count < 0 && (errno == EAGAIN || errno == EINTR))
		return 0;
	if(count < 0)
		fail(http_row_now, "cannot read an answer: %s", strerror(errno));
	if(count == 0) {
		if(keep_alive || !connection->head_done || connection->body_left != 0)
			fail(http_row_now, "the server closed the connection before the whole body arrived");
		return 1;
	}
	if(connection->head_done) {
		if((uint64_t)count > connection->body_left)
			fail(http_row_now, "the body is longer than its Content-Length");
		connection->body_left -= (uint64_t)count;
	} else {
		connection->head_length += (size_t)count;
		connection->head[connection->head_length] = 0;
		const char *end = strstr(connection->head, "\r\n\r\n");
		if(!end && connection->head_length == sizeof(connection->head) - 1)
			fail(http_row_now, "the answer's header is longer than %zu bytes", sizeof(connection->head) - 1);
		if(!end)
			return 0;
		read_head(connection, end, expected);
	}
	return keep_alive && connection->body_left == 0;
}

// One pass of `requests` GETs of /<path> over up to CONNECTIONS
// connections, driven by one poll loop. `expected` is the body's size, or
// 0 when it is not fixed.
static void http_pass(const char *path, int keep_alive, uint64_t requests, uint64_t expected) {
	char request[256];
	snprintf(request, sizeof(request), "GET /%s HTTP/1.1\r\nHost: bench\r\n%s\r\n", path,
	         keep_alive ? "" : "Connection: close\r\n");
	size_t length = strlen(request);
	struct connection connections[CONNECTIONS];
	for(int i = 0; i < CONNECTIONS; i++) {
		connections[i].fd = -1;
		connections[i].connecting = 0;
		connections[i].in_flight = 0;
	}
	uint64_t issued = 0, finished = 0;
	while(finished < requests) {
		struct pollfd fds[CONNECTIONS];
		for(int i = 0; i < CONNECTIONS; i++) {
			struct connection *connection = &connections[i];
			if(!connection->in_flight && issued < requests) {
				issue(connection, request, length);
				issued++;
			}
			fds[i].fd = connection->in_flight ? connection->fd : -1;
			fds[i].events = connection->connecting ? POLLOUT : POLLIN;
			fds[i].revents = 0;
		}
		int ready = poll(fds, CONNECTIONS, 30000);
		if(ready < 0 && errno == EINTR)
			continue;
		if(ready < 0)
			fail(http_row_now, "poll failed: %s", strerror(errno));
		if(ready == 0)
			fail(http_row_now, "no answer within 30 s");
		for(int i = 0; i < CONNECTIONS; i++) {
			struct connection *connection = &connections[i];
			if(!fds[i].revents)
				continue;
			if(connection->connecting) {
				int error = 0;
				socklen_t size = sizeof(error);
				getsockopt(connection->fd, SOL_SOCKET, SO_ERROR, &error, &size);
				if(error != 0)
					fail(http_row_now, "cannot connect to nginx: %s", strerror(error));
				connection->connecting = 0;
				send_request(connection, request, length);
			} else if(read_response(connection, keep_alive, expected)) {
				finished++;
				connection->in_flight = 0;
				if(!keep_alive) {
					close(connection->fd);
					connection->fd = -1;
				}
			}
		}
	}
	for(int i = 0; i < CONNECTIONS; i++) {
		if(connections[i].fd >= 0)
			close(connections[i].fd);
	}
}

// A warm-up pass and PASSES measured ones of an HTTP row; one operation is
// one request.
static void http_row(const char *row, const char *path, int keep_alive, uint64_t requests, uint64_t expected) {
	http_row_now = row;
	http_pass(path, keep_alive, requests, expected);
	for(int pass = 0; pass < PASSES; pass++) {
		uint64_t user_before, system_before, user_after, system_after;
		profile_begin();
		own_time(&user_before, &system_before);
		struct counts before = counts_now();
		uint64_t started = now_ns();
		http_pass(path, keep_alive, requests, expected);
		struct counts after = counts_now();
		own_time(&user_after, &system_after);
		report(row, started, requests);
		profile_end();
		print_counts(row, before, after, requests);
		print_client(row, user_after - user_before, system_after - system_before, requests);
	}
	profile_print(row);
}

static void nginx_rows(void) {
	struct child *nginx = start_nginx();
	http_row("server_nginx_small", "small.html", 1, NGINX_SMALL_REQUESTS, SMALL_SIZE);
	http_row("server_nginx_connect", "small.html", 0, NGINX_CONNECT_REQUESTS, SMALL_SIZE);
	// One operation of the large row is one MiB, which is one request.
	http_row("server_nginx_large_mib", "large.bin", 1, NGINX_LARGE_REQUESTS, LARGE_SIZE);
	stop_server(nginx);
}

// A fixed amount of PHP work for the php CLI row: a sort with a closure,
// string and regular expression functions, a hash, sprintf and a JSON
// round trip. It prints the nanoseconds the passes took.
static const char compute_php[] =
	"<?php\n"
	"$passes = (int)($argv[1] ?? 200);\n"
	"$seed = 12345;\n"
	"$numbers = [];\n"
	"for ($i = 0; $i < 1000; $i++) {\n"
	"    $seed = ($seed * 1103515245 + 12345) & 0x7fffffff;\n"
	"    $numbers[] = $seed % 100000;\n"
	"}\n"
	"$text = str_repeat('the quick brown fox jumps over the lazy dog 42 ', 20);\n"
	"$started = hrtime(true);\n"
	"for ($pass = 0; $pass < $passes; $pass++) {\n"
	"    $sorted = $numbers;\n"
	"    usort($sorted, function ($a, $b) { return $a <=> $b; });\n"
	"    $joined = implode(',', $sorted);\n"
	"    $replaced = str_replace(['quick', 'lazy'], ['slow', 'busy'], $text);\n"
	"    $digits = preg_match_all('/\\d+/', $replaced . $joined, $found);\n"
	"    $hash = md5($joined);\n"
	"    $line = sprintf('%s:%d:%08x', $hash, $digits, $sorted[500]);\n"
	"    $round = json_decode(json_encode(['line' => $line, 'head' => array_slice($sorted, 0, 100)]), true);\n"
	"    if ($round['line'] !== $line) {\n"
	"        fwrite(STDERR, \"the JSON round trip changed the line\\n\");\n"
	"        exit(1);\n"
	"    }\n"
	"}\n"
	"echo 'ns=' . (hrtime(true) - $started) . \"\\n\";\n";

// Runs compute.php once and returns the nanoseconds it printed.
static double compute_ns(void) {
	char *argv[] = {program("net.php.php", "php"), "-n", format("%s/compute.php", work), format("%d", PHP_COMPUTE_PASSES), NULL};
	run_tool("php", argv);
	FILE *file = fopen(format("%s/php.log", work), "r");
	char line[256];
	double ns = 0;
	while(file && fgets(line, sizeof(line), file)) {
		if(!strncmp(line, "ns=", 3))
			ns = strtod(line + 3, NULL);
	}
	if(file)
		fclose(file);
	if(ns <= 0)
		fail("server_php_compute", "compute.php printed no ns= line");
	return ns;
}

static void php_rows(void) {
	struct child *fpm = start_fpm();
	struct child *nginx = start_nginx();
	http_row("server_php_fpm", "page.php", 1, PHP_FPM_REQUESTS, 0);
	stop_server(nginx);
	stop_server(fpm);
	write_file("compute.php", compute_php, sizeof(compute_php) - 1);
	// The first run is the warm-up pass.
	compute_ns();
	for(int pass = 0; pass < PASSES; pass++) {
		profile_begin();
		struct counts before = counts_now();
		double ns = compute_ns();
		struct counts after = counts_now();
		profile_end();
		print_row("server_php_compute", ns / PHP_COMPUTE_PASSES, PHP_COMPUTE_PASSES);
		print_counts("server_php_compute", before, after, PHP_COMPUTE_PASSES);
		print_client("server_php_compute", tool_user_ns, tool_system_ns, PHP_COMPUTE_PASSES);
	}
	profile_print("server_php_compute");
}

// Reads a rate from valkey-benchmark's CSV, which has a line
// `"SET","<requests per second>",...` for each test.
static double csv_rate(const char *name, const char *test) {
	FILE *file = fopen(format("%s/%s.log", work, name), "r");
	if(!file)
		fail(name, "no output");
	char *prefix = format("\"%s\",\"", test);
	char line[512];
	double rate = 0;
	while(fgets(line, sizeof(line), file)) {
		if(!strncmp(line, prefix, strlen(prefix)))
			rate = strtod(line + strlen(prefix), NULL);
	}
	fclose(file);
	if(rate <= 0)
		fail(name, "its output has no %s rate", test);
	return rate;
}

// One valkey-benchmark run of `requests` of each of `tests`, `pipeline`
// of them sent at a time.
static void valkey_benchmark(const char *tests, int requests, int pipeline) {
	char *argv[] = {program("io.valkey.valkey", "valkey-benchmark"), "-h", "127.0.0.1", "-p", format("%d", VALKEY_PORT), "-c", format("%d", CONNECTIONS),
	                "-n", format("%d", requests), "-d", "64", "-r", "100000", "-P", format("%d", pipeline), "-t", (char *)tests, "--csv", NULL};
	run_tool("valkey-benchmark", argv);
}

static void valkey_rows(void) {
	make_dir(format("%s/valkey", work));
	char *argv[] = {program("io.valkey.valkey", "valkey-server"), "--port", format("%d", VALKEY_PORT), "--bind", "127.0.0.1", "--save", "",
	                "--appendonly", "no", "--dir", format("%s/valkey", work), "--daemonize", "no", NULL};
	struct child *valkey = start_server("valkey", SIGTERM, argv);
	wait_ready(valkey, valkey_ready);
	valkey_benchmark("set,get,lpush", VALKEY_REQUESTS, 1);
	for(int pass = 0; pass < PASSES; pass++) {
		struct counts before = counts_now();
		valkey_benchmark("set,get,lpush", VALKEY_REQUESTS, 1);
		struct counts after = counts_now();
		print_row("server_valkey_set", 1e9 / csv_rate("valkey-benchmark", "SET"), VALKEY_REQUESTS);
		print_row("server_valkey_get", 1e9 / csv_rate("valkey-benchmark", "GET"), VALKEY_REQUESTS);
		print_row("server_valkey_lpush", 1e9 / csv_rate("valkey-benchmark", "LPUSH"), VALKEY_REQUESTS);
		print_counts("server_valkey_run", before, after, 3 * (uint64_t)VALKEY_REQUESTS);
		print_client("server_valkey_run", tool_user_ns, tool_system_ns, 3 * (uint64_t)VALKEY_REQUESTS);
	}
	valkey_benchmark("get", VALKEY_PIPELINED_REQUESTS, VALKEY_PIPELINE);
	for(int pass = 0; pass < PASSES; pass++) {
		struct counts before = counts_now();
		valkey_benchmark("get", VALKEY_PIPELINED_REQUESTS, VALKEY_PIPELINE);
		struct counts after = counts_now();
		print_row("server_valkey_get_pipelined", 1e9 / csv_rate("valkey-benchmark", "GET"), VALKEY_PIPELINED_REQUESTS);
		print_counts("server_valkey_get_pipelined", before, after, VALKEY_PIPELINED_REQUESTS);
		print_client("server_valkey_get_pipelined", tool_user_ns, tool_system_ns, VALKEY_PIPELINED_REQUESTS);
	}
	stop_server(valkey);
}

// One pgbench run of `per_client` transactions for each of its clients;
// `select_only` is its -S script. Returns the transactions per second it
// reports without the time to connect.
static double pgbench_tps(int select_only, int per_client) {
	char *argv[] = {program("org.postgresql.postgresql", "pgbench"), "-h", work, "-U", "bench", "-c", format("%d", CONNECTIONS), "-j", "2",
	                "-t", format("%d", per_client), "-b", select_only ? "select-only" : "tpcb-like", "postgres", NULL};
	run_tool("pgbench", argv);
	FILE *file = fopen(format("%s/pgbench.log", work), "r");
	char line[256];
	double tps = 0;
	while(file && fgets(line, sizeof(line), file)) {
		const char *at = strstr(line, "tps = ");
		if(at && strstr(line, "without initial connection time"))
			tps = strtod(at + 6, NULL);
	}
	if(file)
		fclose(file);
	if(tps <= 0)
		fail("pgbench", "its output has no tps line");
	return tps;
}

static void pgbench_row(const char *row, int select_only, int per_client) {
	pgbench_tps(select_only, per_client);
	uint64_t rounds = (uint64_t)CONNECTIONS * (uint64_t)per_client;
	for(int pass = 0; pass < PASSES; pass++) {
		profile_begin();
		struct counts before = counts_now();
		double tps = pgbench_tps(select_only, per_client);
		struct counts after = counts_now();
		profile_end();
		print_row(row, 1e9 / tps, rounds);
		print_counts(row, before, after, rounds);
		print_client(row, tool_user_ns, tool_system_ns, rounds);
	}
	profile_print(row);
}

// Prints each line of `path` after "TRACE <row> ".
static void print_traced(const char *row, const char *path) {
	FILE *file = fopen(path, "r");
	char line[512];
	while(file && fgets(line, sizeof(line), file))
		printf("TRACE %s %s%s", row, line, strchr(line, '\n') ? "" : "\n");
	if(file)
		fclose(file);
	fflush(stdout);
}

// With SERVERBENCH_TRACE set and rtrace there (Radix), one more pass of a
// pgbench row against a server started under rtrace, which counts the
// calls of the server and of every process it starts (the backends, the
// checkpointer, the writers) from its start to its end; prints the table,
// what the machine did during the pass, and the pass's transactions per
// second, as TRACE lines. rtrace starts the server, since Radix lets a
// process trace only those below it (ADR 0214). `server` is the server's
// command, which `postgresql_rows` has stopped.
static void traced_pgbench_pass(const char *row, char *const server[], int select_only, int per_client) {
	static const char rtrace[] = "/system/commands/rtrace";
	if(!getenv("SERVERBENCH_TRACE") || access(rtrace, X_OK) != 0)
		return;
	char *table = format("%s/trace-%s.txt", work, row);
	char *argv[40] = {(char *)rtrace, "-c", "-f", "-q", "-o", table};
	size_t count = 6;
	for(size_t i = 0; server[i] && count < sizeof(argv) / sizeof(argv[0]) - 1; i++)
		argv[count++] = server[i];
	argv[count] = NULL;
	// Stopped by its own end: rtrace ends when the server does.
	struct child *traced = start_server("postgres-traced", 0, argv);
	wait_ready(traced, postgres_ready);
	uint64_t rounds = (uint64_t)CONNECTIONS * (uint64_t)per_client;
	struct counts before = counts_now();
	double tps = pgbench_tps(select_only, per_client);
	struct counts after = counts_now();
	FILE *pid_file = fopen(format("%s/pg/postmaster.pid", work), "r");
	long postmaster = 0;
	if(pid_file) {
		if(fscanf(pid_file, "%ld", &postmaster) != 1)
			postmaster = 0;
		fclose(pid_file);
	}
	if(postmaster > 0)
		kill((pid_t)postmaster, SIGINT);
	stop_server(traced);
	printf("TRACE %s traced pass: %.1f ns per transaction, %llu transactions; pgbench user_ns=%llu system_ns=%llu\n", row, 1e9 / tps,
	       (unsigned long long)rounds, (unsigned long long)tool_user_ns, (unsigned long long)tool_system_ns);
	if(before.valid && after.valid && after.radix)
		printf("TRACE %s machine: switches=%llu user_ns=%llu system_ns=%llu faults=%llu preemptions=%llu\n", row,
		       (unsigned long long)(after.switches - before.switches), (unsigned long long)(after.user_ns - before.user_ns),
		       (unsigned long long)(after.system_ns - before.system_ns), (unsigned long long)(after.faults - before.faults),
		       (unsigned long long)(after.preemptions - before.preemptions));
	print_traced(row, table);
	print_traced(row, format("%s/postgres-traced.log", work));
}

static void postgresql_rows(void) {
	char *initdb[] = {program("org.postgresql.postgresql", "initdb"), "-D", format("%s/pg", work), "-U", "bench", "--auth=trust", "--no-sync",
	                  "--no-locale", "-E", "UTF8", NULL};
	run_tool("initdb", initdb);
	char *argv[] = {program("org.postgresql.postgresql", "postgres"), "-D", format("%s/pg", work), "-k", work,
	                "-c", "listen_addresses=", "-c", "shared_buffers=128MB", "-c", "max_connections=20", "-c", "huge_pages=off",
	                "-c", "checkpoint_flush_after=0", "-c", "bgwriter_flush_after=0", "-c", "backend_flush_after=0",
	                "-c", "dynamic_shared_memory_type=posix", "-c", "timezone=UTC", "-c", "log_timezone=UTC",
	                "-c", "fsync=on", "-c", "synchronous_commit=on", NULL};
	struct child *postgres = start_server("postgres", SIGINT, argv);
	wait_ready(postgres, postgres_ready);
	char *load[] = {program("org.postgresql.postgresql", "pgbench"), "-h", work, "-U", "bench", "-i", "-s", "10", "-q", "postgres", NULL};
	run_tool("pgbench", load);
	pgbench_row("server_postgresql_select", 1, PGBENCH_SELECT_PER_CLIENT);
	pgbench_row("server_postgresql_tpcb", 0, PGBENCH_TPCB_PER_CLIENT);
	stop_server(postgres);
	traced_pgbench_pass("server_postgresql_select", argv, 1, PGBENCH_SELECT_PER_CLIENT);
	traced_pgbench_pass("server_postgresql_tpcb", argv, 0, PGBENCH_TPCB_PER_CLIENT);
}

// A chain of additions, each waiting for the one before, so one clock
// cycle each on any x86-64 processor: the time of one is the processor's
// clock cycle, which says how fast each system runs it. As bench.c's row
// of the same name, the chain first runs for 250 ms, so that a processor
// that lets its clock fall when idle has climbed back to the clock the
// rows run at; the lab compares the two systems' clocks from it before
// it compares any row (ADR 0557).
static void cpu_cycle(uint64_t rounds) {
#ifdef __x86_64__
	uint64_t x = 0, warmed = 0;
	uint64_t warm_until = now_ns() + 250 * 1000000ull;
	while(now_ns() < warm_until) {
		for(int i = 0; i < 100000; i++)
			__asm__ volatile(".rept 100\n\taddq $1, %0\n\t.endr" : "+r"(x));
		warmed += 100000;
	}
	uint64_t started = now_ns();
	for(uint64_t i = 0; i < rounds; i++)
		__asm__ volatile(".rept 100\n\taddq $1, %0\n\t.endr" : "+r"(x));
	report("cpu_cycle", started, rounds * 100);
	if(x != (warmed + rounds) * 100)
		fail("cpu_cycle", "the sum is wrong");
#else
	(void)rounds;
#endif
}

// The C library's allocator, as the servers use it: each operation frees
// the oldest of MALLOC_LIVE blocks and allocates one of a size from 16 to
// 512 bytes, as a server's requests make and drop their small objects.
// server_malloc_one runs it on one thread, server_malloc_two on two at
// once, which share the allocator as pgbench's two threads do. Neither
// row is held to Linux's: they say how much of a server's time its C
// library's allocator could account for.
#define MALLOC_LIVE 1024
#define MALLOC_ROUNDS 2000000

static void *malloc_rounds(void *argument) {
	(void)argument;
	static const size_t sizes[] = {16, 24, 32, 48, 64, 96, 128, 256, 512};
	void *live[MALLOC_LIVE] = {0};
	uint32_t seed = 12345;
	for(uint64_t i = 0; i < MALLOC_ROUNDS; i++) {
		seed = seed * 1103515245u + 12345u;
		size_t at = i % MALLOC_LIVE;
		free(live[at]);
		live[at] = malloc(sizes[(seed >> 16) % (sizeof(sizes) / sizeof(sizes[0]))]);
		if(!live[at])
			fail("server_malloc", "out of memory");
		*(volatile char *)live[at] = 1;
	}
	for(size_t at = 0; at < MALLOC_LIVE; at++)
		free(live[at]);
	return NULL;
}

// The event rows' pipes: 1,000 idle ones and the active one, last, as
// bench.c's posix_events_idle has them.
#define EVENTS_IDLE_PIPES 1000
#define EVENTS_ROUNDS 200000

// An event queue's wait for one ready pipe among 1,000 idle ones, each
// watched with edge triggering, as bench.c's posix_events_idle times it,
// profiled: a round writes a byte to the active pipe, waits without
// blocking for the one event that makes, and reads the byte.
static void events_rows(void) {
	static int ends[EVENTS_IDLE_PIPES + 1][2];
	struct rlimit files;
	if(getrlimit(RLIMIT_NOFILE, &files) != 0)
		fail("server_events_idle", "getrlimit: %s", strerror(errno));
	files.rlim_cur = files.rlim_max;
	if(setrlimit(RLIMIT_NOFILE, &files) != 0)
		fail("server_events_idle", "setrlimit: %s", strerror(errno));
	for(int i = 0; i <= EVENTS_IDLE_PIPES; i++) {
		if(pipe(ends[i]) != 0)
			fail("server_events_idle", "pipe: %s", strerror(errno));
	}
#ifdef __linux__
	int queue = epoll_create1(0);
	if(queue < 0)
		fail("server_events_idle", "epoll_create1: %s", strerror(errno));
	for(int i = 0; i <= EVENTS_IDLE_PIPES; i++) {
		struct epoll_event watched = {.events = EPOLLIN | EPOLLET};
		watched.data.fd = ends[i][0];
		if(epoll_ctl(queue, EPOLL_CTL_ADD, ends[i][0], &watched) != 0)
			fail("server_events_idle", "epoll_ctl: %s", strerror(errno));
	}
	struct epoll_event ready[64];
#else
	int queue = kqueue();
	if(queue < 0)
		fail("server_events_idle", "kqueue: %s", strerror(errno));
	static struct kevent changes[EVENTS_IDLE_PIPES + 1];
	for(int i = 0; i <= EVENTS_IDLE_PIPES; i++)
		EV_SET(&changes[i], ends[i][0], EVFILT_READ, EV_ADD | EV_CLEAR, 0, 0, NULL);
	if(kevent(queue, changes, EVENTS_IDLE_PIPES + 1, NULL, 0, NULL) != 0)
		fail("server_events_idle", "kevent: %s", strerror(errno));
	struct kevent ready[64];
	const struct timespec no_wait = {0, 0};
#endif
	int active = EVENTS_IDLE_PIPES;
	char byte;
	// A warm-up pass, then the measured ones.
	for(int pass = -1; pass < PASSES; pass++) {
		if(pass >= 0)
			profile_begin();
		struct counts before = counts_now();
		uint64_t started = now_ns();
		for(uint64_t i = 0; i < EVENTS_ROUNDS; i++) {
			if(write(ends[active][1], "x", 1) != 1)
				fail("server_events_idle", "write: %s", strerror(errno));
#ifdef __linux__
			if(epoll_wait(queue, ready, 64, 0) != 1)
				fail("server_events_idle", "epoll_wait did not find the one ready pipe");
#else
			if(kevent(queue, NULL, 0, ready, 64, &no_wait) != 1)
				fail("server_events_idle", "kevent did not find the one ready pipe");
#endif
			if(read(ends[active][0], &byte, 1) != 1)
				fail("server_events_idle", "read: %s", strerror(errno));
		}
		if(pass < 0)
			continue;
		report("server_events_idle", started, EVENTS_ROUNDS);
		profile_end();
		print_counts("server_events_idle", before, counts_now(), EVENTS_ROUNDS);
	}
	profile_print("server_events_idle");
	close(queue);
	for(int i = 0; i <= EVENTS_IDLE_PIPES; i++) {
		close(ends[i][0]);
		close(ends[i][1]);
	}
}

static void malloc_rows(void) {
	malloc_rounds(NULL);
	uint64_t started = now_ns();
	malloc_rounds(NULL);
	report("server_malloc_one", started, MALLOC_ROUNDS);
	pthread_t other;
	started = now_ns();
	if(pthread_create(&other, NULL, malloc_rounds, NULL) != 0)
		fail("server_malloc", "cannot start a thread");
	malloc_rounds(NULL);
	pthread_join(other, NULL);
	report("server_malloc_two", started, MALLOC_ROUNDS);
}

// Makes the work folder. As root it gives the folder to `bench` and
// becomes that account, with no groups of the root account's, and checks
// that root cannot be had back.
static void make_work(void) {
	struct passwd *account = NULL;
	if(geteuid() == 0) {
		account = getpwnam("bench");
		if(!account)
			fail("start", "there is no account bench; run it as an account other than root");
	}
	if(!mkdtemp(work))
		fail("work folder", "cannot make %s: %s", work, strerror(errno));
	work_made = 1;
	if(!account)
		return;
	if(chown(work, account->pw_uid, account->pw_gid) != 0)
		fail("work folder", "cannot give %s to bench: %s", work, strerror(errno));
	if(setgroups(0, NULL) != 0 || setgid(account->pw_gid) != 0 || setuid(account->pw_uid) != 0)
		fail("start", "cannot become bench: %s", strerror(errno));
	if(setuid(0) == 0)
		fail("start", "the process can become root again");
}

// What the servers and tools run with, which is all they are given.
static void make_environment(void) {
	const char *path = getenv("PATH");
	environment[0] = "TZ=UTC";
	environment[1] = "LC_ALL=C";
	environment[2] = format("PATH=%s", path ? path : "/usr/bin:/bin");
	environment[3] = format("HOME=%s", work);
	environment[4] = NULL;
}

// The programs a server needs.
static const char *const nginx_programs[][2] = {{"org.nginx.nginx", "nginx"}};
static const char *const php_programs[][2] = {{"net.php.php", "php"}, {"net.php.php", "php-fpm"}, {"org.nginx.nginx", "nginx"}};
static const char *const valkey_programs[][2] = {{"io.valkey.valkey", "valkey-server"}, {"io.valkey.valkey", "valkey-benchmark"}};
static const char *const postgresql_programs[][2] = {{"org.postgresql.postgresql", "initdb"}, {"org.postgresql.postgresql", "postgres"},
                                                     {"org.postgresql.postgresql", "pgbench"}, {"org.postgresql.postgresql", "pg_isready"}};

static void require(const char *bundle, const char *name) {
	char *path = program(bundle, name);
	if(access(path, X_OK) != 0) {
		printf("serverbench: %s is not installed\n", path);
		fflush(stdout);
		exit(1);
	}
}

static void require_all(const char *const (*programs)[2], size_t count) {
	for(size_t i = 0; i < count; i++)
		require(programs[i][0], programs[i][1]);
}

#define REQUIRE_ALL(programs) require_all(programs, sizeof(programs) / sizeof(programs[0]))

int main(int argc, char **argv) {
	int first = 1;
	if(argc > 2 && !strcmp(argv[1], "--applications")) {
		applications = argv[2];
		first = 3;
	}
	if(first >= argc) {
		printf("serverbench: usage: serverbench [--applications DIR] nginx|php|valkey|postgresql...\n");
		return 2;
	}
	for(int i = first; i < argc; i++) {
		if(!strcmp(argv[i], "nginx"))
			REQUIRE_ALL(nginx_programs);
		else if(!strcmp(argv[i], "php"))
			REQUIRE_ALL(php_programs);
		else if(!strcmp(argv[i], "valkey"))
			REQUIRE_ALL(valkey_programs);
		else if(!strcmp(argv[i], "postgresql"))
			REQUIRE_ALL(postgresql_programs);
		else if(!strcmp(argv[i], "events"))
			continue;
		else {
			printf("serverbench: %s is not a server; the servers are nginx, php, valkey and postgresql\n", argv[i]);
			return 2;
		}
	}
	// A write to a connection the server closed is an error to report, not
	// the end of this program.
	signal(SIGPIPE, SIG_IGN);
	atexit(cleanup);
	profile_open();
	make_work();
	make_environment();
	cpu_cycle(1000000);
	malloc_rows();
	for(int i = first; i < argc; i++) {
		if(!strcmp(argv[i], "nginx"))
			nginx_rows();
		else if(!strcmp(argv[i], "php"))
			php_rows();
		else if(!strcmp(argv[i], "valkey"))
			valkey_rows();
		else if(!strcmp(argv[i], "events"))
			events_rows();
		else
			postgresql_rows();
	}
	printf("serverbench: done\n");
	fflush(stdout);
	return 0;
}
