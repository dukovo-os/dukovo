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
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

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
		own_time(&user_before, &system_before);
		struct counts before = counts_now();
		uint64_t started = now_ns();
		http_pass(path, keep_alive, requests, expected);
		struct counts after = counts_now();
		own_time(&user_after, &system_after);
		report(row, started, requests);
		print_counts(row, before, after, requests);
		print_client(row, user_after - user_before, system_after - system_before, requests);
	}
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
		struct counts before = counts_now();
		double ns = compute_ns();
		struct counts after = counts_now();
		print_row("server_php_compute", ns / PHP_COMPUTE_PASSES, PHP_COMPUTE_PASSES);
		print_counts("server_php_compute", before, after, PHP_COMPUTE_PASSES);
		print_client("server_php_compute", tool_user_ns, tool_system_ns, PHP_COMPUTE_PASSES);
	}
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
		struct counts before = counts_now();
		double tps = pgbench_tps(select_only, per_client);
		struct counts after = counts_now();
		print_row(row, 1e9 / tps, rounds);
		print_counts(row, before, after, rounds);
		print_client(row, tool_user_ns, tool_system_ns, rounds);
	}
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
		else {
			printf("serverbench: %s is not a server; the servers are nginx, php, valkey and postgresql\n", argv[i]);
			return 2;
		}
	}
	// A write to a connection the server closed is an error to report, not
	// the end of this program.
	signal(SIGPIPE, SIG_IGN);
	atexit(cleanup);
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
		else
			postgresql_rows();
	}
	printf("serverbench: done\n");
	fflush(stdout);
	return 0;
}
