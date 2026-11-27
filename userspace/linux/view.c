// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a dynamically linked glibc program finds in the sandbox that linux(1)
// builds for it (ADRs 0618 to 0622): a /proc and /sys in Linux's text, the
// program's own path, a standard output that /proc/self/fd/1 leads back to,
// a /proc/self/fd that lists the descriptors open, its own among them,
// the network interfaces through a netlink route socket, the accounts in
// /etc, a resolver file, the kernel's name, and the person's own folders
// kept out of sight, and a /dev/shm whose files two processes map and
// share. A call that the Linux table does not handle is
// answered by linuxd, which the case `personality` shows with the one call
// that only linuxd answers, and which the case `restart` ends and waits for
// the supervisor to start again.
//
// Each case prints one line, `view: <case> ok`, or says what failed and
// with which errno, or `view: <case> skip (<why>)` when the case has
// nothing to run on. The program ends with `view: all <N> cases passed on
// Dukovo` and exits with 0, or exits with 1 after saying how many cases
// failed.
//
//   view [--restart] [--linuxd PID] [CASE...]
//
// With case names it runs only those. `--restart` adds the case `restart`:
// it ends linuxd with SIGKILL, which the program finds under /proc by the
// name `linuxd` in its comm file, unless `--linuxd` names its process
// number, and waits until a forwarded call is answered again.
//
// It passes on a Linux machine, which is how its expectations are known to
// be Linux's, but for the cases that check the sandbox: `hidden` and
// `restart` say `skip` there. The program takes itself to be on Dukovo when
// `uname -v` has "Dukovo" in it, as the kernel's Linux table writes its
// version (`#1 SMP Dukovo <release>`); the first line it prints says which.
// Run it by its path: the cases that name the program compare its path with
// the one the system gives. Standard output is a terminal or a pipe, as the
// boot test and the lab test have it: /proc/self/fd/1 of a file opens it
// again from its start, on Linux too, and the case `fd_one` would write
// over what the others printed.
//
//   cc -std=c11 -D_GNU_SOURCE -O2 -Wall -Wextra -o view view.c

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <ifaddrs.h>
#include <limits.h>
#include <pwd.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/personality.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

// The argument that asks personality(2) for the persona without changing
// it, and the persona Linux gives every program that has not asked for
// another.
#define PERSONA_QUERY 0xffffffffUL
#define PERSONA_LINUX 0

// The most a file of /proc or /sys is read in, which is far more than any
// the cases read holds.
#define READ_MAX (4UL << 20)

// How long the case `restart` waits for linuxd to answer again, in seconds,
// and how long it lets one call wait: a call queued for a linuxd that is
// gone is cut short by the alarm, so that the case fails by name instead of
// holding the boot test.
#define RESTART_SECONDS 30
#define CALL_SECONDS 5

// The name of the case that runs, which every line it prints starts with,
// how many checks of it failed, and whether it had nothing to run on.
static const char *case_name = "";
static int case_failures;
static int case_skipped;

// Whether the system says it is Dukovo, in the version `uname -v` gives.
static int on_dukovo;

// This program's path as it was started, and the same made absolute, for
// the cases that compare it with the system's own idea of it.
static char given_path[PATH_MAX];
static char real_path[PATH_MAX];

// The process number the option --linuxd gave, or 0.
static pid_t linuxd_number;

// Says that a check failed, with its line and the errno it saw, and counts it.
static void check_failed(int line, const char *what, int error) {
	printf("view: %s: FAILED: view.c:%d: %s (errno %d: %s)\n", case_name, line, what, error,
			strerror(error));
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

// Fails the running case, saying `what` is wrong.
#define FAIL(what) check_failed(__LINE__, what, errno)

// Says that the running case has nothing to run on.
static void skip(const char *why) {
	printf("view: %s skip (%s)\n", case_name, why);
	fflush(stdout);
	case_skipped = 1;
}

// Reads the whole of the file `path` into memory that the caller frees, and
// ends it with a NUL, or returns NULL with errno as the failed call left it.
// A file of /proc or /sys says it is empty before it is read, so the read
// goes on to its end instead of to its size.
static char *read_file(const char *path) {
	int fd = open(path, O_RDONLY | O_CLOEXEC);
	if(fd < 0)
		return NULL;
	char *text = NULL;
	size_t used = 0, capacity = 0;
	for(;;) {
		if(capacity - used < 2) {
			if(capacity >= READ_MAX) {
				errno = EFBIG;
				goto failed;
			}
			capacity = capacity ? capacity * 2 : 4096;
			char *bigger = realloc(text, capacity);
			if(!bigger)
				goto failed;
			text = bigger;
		}
		ssize_t got = read(fd, text + used, capacity - used - 1);
		if(got < 0 && errno == EINTR)
			continue;
		if(got < 0)
			goto failed;
		if(got == 0)
			break;
		used += (size_t)got;
	}
	close(fd);
	text[used] = 0;
	return text;
failed:;
	int error = errno;
	free(text);
	close(fd);
	errno = error;
	return NULL;
}

// The first line of `text` that starts with `prefix`, or NULL.
static const char *find_line(const char *text, const char *prefix) {
	size_t length = strlen(prefix);
	for(const char *line = text; line; ) {
		if(!strncmp(line, prefix, length))
			return line;
		line = strchr(line, '\n');
		if(line)
			line++;
	}
	return NULL;
}

// Whether `path` is a folder that opendir opens.
static int opens(const char *path) {
	DIR *folder = opendir(path);
	if(!folder)
		return 0;
	closedir(folder);
	return 1;
}

// Whether /devices, if the view holds it, has no more than a terminal's
// devices: tty, ptmx and pts.
static int only_terminal_devices(void) {
	DIR *folder = opendir("/devices");
	if(!folder)
		return 1;
	int only = 1;
	for(struct dirent *each; (each = readdir(folder));) {
		const char *name = each->d_name;
		if(strcmp(name, ".") && strcmp(name, "..") && strcmp(name, "tty") &&
		   strcmp(name, "ptmx") && strcmp(name, "pts"))
			only = 0;
	}
	closedir(folder);
	return only;
}

// Whether /system/commands holds no more of Dukovo's programs than the
// shell and the core commands that /bin and /usr/bin name (ADR 0626).
static int only_shell_commands(void) {
	DIR *folder = opendir("/system/commands");
	if(!folder)
		return 1;
	int only = 1;
	for(struct dirent *each; (each = readdir(folder));) {
		const char *name = each->d_name;
		if(strcmp(name, ".") && strcmp(name, "..") && strcmp(name, "nukesh") &&
		   strcmp(name, "coreutils") && strcmp(name, "find") && strcmp(name, "xargs"))
			only = 0;
	}
	closedir(folder);
	return only;
}

// Whether `text` is a list of CPU numbers and ranges as the kernel writes
// one: digits, `-` and `,`, ending with a newline.
static int is_cpu_list(const char *text) {
	size_t length = strlen(text);
	if(length < 2 || text[length - 1] != '\n' || text[0] < '0' || text[0] > '9')
		return 0;
	for(size_t at = 0; at + 1 < length; at++)
		if(!((text[at] >= '0' && text[at] <= '9') || text[at] == '-' || text[at] == ','))
			return 0;
	return 1;
}

// The path the system says this program was started by.
static ssize_t own_path(char *buffer, size_t size) {
	ssize_t length = readlink("/proc/self/exe", buffer, size - 1);
	if(length >= 0)
		buffer[length] = 0;
	return length;
}

// /proc/self/maps names the program and the stack. The program's path is
// the one /proc/self/exe gives, so that the case does not depend on the
// other one's answer.
static void maps_case(void) {
	char exe[PATH_MAX];
	CHECK(own_path(exe, sizeof(exe)) > 0);
	char *maps = read_file("/proc/self/maps");
	CHECK(maps != NULL);
	if(!maps)
		return;
	CHECK(strstr(maps, exe) != NULL);
	CHECK(strstr(maps, "[stack]") != NULL);
	free(maps);
}

// /proc/cpuinfo has a `processor` line for a CPU.
static void cpuinfo_case(void) {
	char *info = read_file("/proc/cpuinfo");
	CHECK(info != NULL);
	if(!info)
		return;
	CHECK(find_line(info, "processor") != NULL);
	free(info);
}

// /proc/self/status says this process's number as getpid does.
static void status_case(void) {
	char *status = read_file("/proc/self/status");
	CHECK(status != NULL);
	if(!status)
		return;
	const char *line = find_line(status, "Pid:");
	int pid = 0;
	CHECK(line != NULL && sscanf(line, "Pid: %d", &pid) == 1);
	CHECK(pid == (int)getpid());
	free(status);
}

// /proc/self/exe is the path the program was started by: Linux gives the
// real path, Dukovo the one it was started by (radix-linux(7)), which are
// one when no folder of the path is a link.
static void exe_case(void) {
	char exe[PATH_MAX];
	CHECK(own_path(exe, sizeof(exe)) > 0);
	CHECK(!strcmp(exe, real_path) || !strcmp(exe, given_path));
}

// /proc/self/fd/1 opens, and what is written to it reaches standard
// output, where the line shows beside the others.
static void fd_one_case(void) {
	static const char line[] = "view: wrote through /proc/self/fd/1\n";
	int fd = open("/proc/self/fd/1", O_WRONLY);
	CHECK(fd >= 0);
	if(fd < 0)
		return;
	CHECK(write(fd, line, sizeof(line) - 1) == (ssize_t)(sizeof(line) - 1));
	CHECK(close(fd) == 0);
}

// getifaddrs, which glibc makes from netlink route dumps, lists the
// loopback interface.
static void ifaddrs_case(void) {
	struct ifaddrs *list;
	CHECK(getifaddrs(&list) == 0);
	if(!list)
		return;
	int found = 0;
	for(const struct ifaddrs *each = list; each; each = each->ifa_next)
		if(!strcmp(each->ifa_name, "lo"))
			found = 1;
	freeifaddrs(list);
	CHECK(found);
}

// The account of the user the program runs as is in /etc/passwd, which the
// C library reads through nsswitch.conf and libnss_files.
static void passwd_case(void) {
	errno = 0;
	const struct passwd *entry = getpwuid(getuid());
	CHECK(entry != NULL);
	if(!entry)
		return;
	CHECK(entry->pw_uid == getuid());
	CHECK(entry->pw_name[0] != 0);
}

// The resolver's file opens.
static void resolv_case(void) {
	int fd = open("/etc/resolv.conf", O_RDONLY);
	CHECK(fd >= 0);
	if(fd >= 0)
		CHECK(close(fd) == 0);
}

// uname says Linux, as a program that tells systems apart by it expects.
static void uname_case(void) {
	struct utsname name;
	CHECK(uname(&name) == 0);
	CHECK(!strcmp(name.sysname, "Linux"));
	CHECK(!strcmp(name.machine, "x86_64"));
}

// The CPUs that are online, as /sys gives them.
static void cpu_online_case(void) {
	char *online = read_file("/sys/devices/system/cpu/online");
	CHECK(online != NULL);
	if(!online)
		return;
	CHECK(is_cpu_list(online));
	free(online);
}

// A file of /dev/shm that shm_open makes is shared by the processes that
// map it: a child writes through its mapping, and its parent reads that
// through its own, as glibc's named semaphores and LTP's library rely on.
// The file is /dev/shm's, by its name there, and goes with shm_unlink.
static void shm_case(void) {
	char name[64];
	char path[80];
	snprintf(name, sizeof(name), "/view-shm-%ld", (long)getpid());
	snprintf(path, sizeof(path), "/dev/shm%s", name);
	int fd = shm_open(name, O_CREAT | O_EXCL | O_RDWR, 0600);
	CHECK(fd >= 0);
	if(fd < 0)
		return;
	CHECK(ftruncate(fd, 4096) == 0);
	struct stat status;
	CHECK(stat(path, &status) == 0 && status.st_size == 4096);
	volatile int *word = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	CHECK(word != MAP_FAILED);
	CHECK(close(fd) == 0);
	if(word != MAP_FAILED) {
		*word = 1;
		pid_t child = fork();
		CHECK(child >= 0);
		if(child == 0) {
			*word = 0x5eed;
			_exit(0);
		}
		int how = 0;
		CHECK(child < 0 || waitpid(child, &how, 0) == child);
		CHECK(WIFEXITED(how) && WEXITSTATUS(how) == 0);
		CHECK(*word == 0x5eed);
		CHECK(munmap((void *)word, 4096) == 0);
	}
	CHECK(shm_unlink(name) == 0);
	CHECK(access(path, F_OK) != 0 && errno == ENOENT);
}

// The person's own folders are not in the view. Of Dukovo's programs only
// the shell and the core commands are, for /bin/sh and the commands of
// /usr/bin (ADR 0626), and none of its settings; /devices holds a
// terminal's devices alone, which the links of /dev name; and the parent of
// the person's home is there only as the way to what the view holds beneath
// it, so it opens to no more than the home's own name: the other accounts'
// homes are not in it. The program's HOME is a folder of the person's
// app-data, from which the home is found.
static void hidden_case(void) {
	if(!on_dukovo) {
		skip("it checks the sandbox, which only Dukovo makes");
		return;
	}
	CHECK(only_shell_commands());
	CHECK(!opens("/system/settings"));
	CHECK(only_terminal_devices());
	const char *data = getenv("HOME");
	const char *app_data = data ? strstr(data, "/app-data/") : NULL;
	CHECK(app_data != NULL);
	if(!app_data)
		return;
	char home[PATH_MAX];
	snprintf(home, sizeof(home), "%.*s", (int)(app_data - data), data);
	char *name = strrchr(home, '/');
	CHECK(name != NULL);
	if(!name)
		return;
	*name++ = 0;
	DIR *parent = opendir(home[0] ? home : "/");
	if(!parent)
		return;
	for(const struct dirent *entry; (entry = readdir(parent)); ) {
		if(!strcmp(entry->d_name, ".") || !strcmp(entry->d_name, ".."))
			continue;
		if(strcmp(entry->d_name, name)) {
			char what[PATH_MAX];
			snprintf(what, sizeof(what), "the parent of the home lists %s, which is in the view",
					entry->d_name);
			errno = 0;
			FAIL(what);
		}
	}
	closedir(parent);
}

// personality(2) is a call that only linuxd answers, in the view: every
// program has the persona Linux gives, and asking for it changes nothing.
static void personality_case(void) {
	CHECK(personality(PERSONA_QUERY) == PERSONA_LINUX);
}

// Finds the process number of a process called linuxd under /proc, other
// than `except`, or returns 0. Its comm file holds the name, as Linux's do.
static pid_t find_linuxd(pid_t except) {
	DIR *proc = opendir("/proc");
	if(!proc)
		return 0;
	pid_t found = 0;
	for(const struct dirent *entry; !found && (entry = readdir(proc)); ) {
		char *end;
		long number = strtol(entry->d_name, &end, 10);
		if(*end || number <= 0 || number == except)
			continue;
		char path[64];
		snprintf(path, sizeof(path), "/proc/%ld/comm", number);
		char *comm = read_file(path);
		if(comm && !strcmp(comm, "linuxd\n"))
			found = (pid_t)number;
		free(comm);
	}
	closedir(proc);
	return found;
}

// Ends the wait of a call that no linuxd takes: the signal makes the call
// fail with EINTR, as the handler has no SA_RESTART.
static void on_alarm(int signal_number) {
	(void)signal_number;
}

// personality(2) with at most CALL_SECONDS to be answered.
static long persona_within_limit(void) {
	alarm(CALL_SECONDS);
	long persona = personality(PERSONA_QUERY);
	alarm(0);
	return persona;
}

// The monotonic clock, in milliseconds.
static long long now_ms(void) {
	struct timespec now;
	clock_gettime(CLOCK_MONOTONIC, &now);
	return (long long)now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

// Sleeps for `ms` milliseconds, however often a signal cuts it short.
static void sleep_ms(long ms) {
	struct timespec left = {ms / 1000, (ms % 1000) * 1000000L};
	while(nanosleep(&left, &left) < 0 && errno == EINTR) {
	}
}

// Lists the descriptors of /proc/self/fd through the directory's own
// handle, as closefrom(3)'s fallback and Python's subprocess do: standard
// input, output and error are there, and so is the directory itself.
// Returns how many checks failed.
static int list_own_descriptors(void) {
	DIR *dir = opendir("/proc/self/fd");
	CHECK(dir != NULL);
	if(!dir)
		return case_failures;
	int own = dirfd(dir);
	int found = 0;
	errno = 0;
	for(const struct dirent *entry; (entry = readdir(dir));) {
		if(entry->d_name[0] == '.')
			continue;
		int fd = atoi(entry->d_name);
		if(fd <= 2)
			found |= 1 << fd;
		else if(fd == own)
			found |= 1 << 3;
	}
	CHECK(errno == 0);
	CHECK(found == 0xf);
	CHECK(closedir(dir) == 0);
	return case_failures;
}

// /proc/self/fd lists the descriptors that are open, its own among them.
// linuxd makes the listing from the kernel's list of the process's files,
// which holds a file of linuxd's own /proc: the directory. The listing runs
// in a child that is ended after CALL_SECONDS, so that a listing that never
// comes fails the case by name rather than holding the test.
static void fd_list_case(void) {
	fflush(stdout);
	pid_t child = fork();
	CHECK(child >= 0);
	if(child < 0)
		return;
	if(child == 0)
		_exit(list_own_descriptors() ? 1 : 0);
	int how = 0;
	pid_t ended = 0;
	long long limit = now_ms() + CALL_SECONDS * 1000LL;
	while((ended = waitpid(child, &how, WNOHANG)) == 0 && now_ms() < limit)
		sleep_ms(20);
	if(ended == 0) {
		FAIL("/proc/self/fd was not listed in time");
		kill(child, SIGKILL);
		waitpid(child, &how, 0);
		return;
	}
	CHECK(ended == child);
	CHECK(WIFEXITED(how) && WEXITSTATUS(how) == 0);
}

// linuxd is ended with SIGKILL while the program runs, and the supervisor
// starts it again on the same connection and endpoint: a forwarded call is
// answered again within RESTART_SECONDS by another process called linuxd,
// and /proc, which it serves, still answers.
static void restart_case(void) {
	if(!on_dukovo) {
		skip("it ends linuxd, which only Dukovo has");
		return;
	}
	struct sigaction action;
	memset(&action, 0, sizeof(action));
	action.sa_handler = on_alarm;
	sigemptyset(&action.sa_mask);
	CHECK(sigaction(SIGALRM, &action, NULL) == 0);
	pid_t old = linuxd_number ? linuxd_number : find_linuxd(0);
	if(old <= 0) {
		FAIL("no process called linuxd is under /proc, and --linuxd names none");
		return;
	}
	CHECK(persona_within_limit() == PERSONA_LINUX);
	CHECK(kill(old, SIGKILL) == 0);
	long persona = -1;
	pid_t new = 0;
	long long limit = now_ms() + RESTART_SECONDS * 1000LL;
	while(now_ms() < limit) {
		persona = persona_within_limit();
		new = find_linuxd(old);
		if(persona == PERSONA_LINUX && new)
			break;
		sleep_ms(100);
	}
	if(persona != PERSONA_LINUX)
		FAIL("personality is not answered after linuxd was ended");
	if(!new)
		FAIL("no other process called linuxd is under /proc after it was ended");
	char *status = read_file("/proc/self/status");
	CHECK(status != NULL);
	if(status)
		CHECK(find_line(status, "Pid:") != NULL);
	free(status);
}

// A case's name and the function that runs it.
struct test_case {
	const char *name;
	void (*run)(void);
};

static const struct test_case CASES[] = {
	{"maps", maps_case},
	{"cpuinfo", cpuinfo_case},
	{"status", status_case},
	{"exe", exe_case},
	{"fd_one", fd_one_case},
	{"ifaddrs", ifaddrs_case},
	{"passwd", passwd_case},
	{"resolv", resolv_case},
	{"uname", uname_case},
	{"cpu_online", cpu_online_case},
	{"hidden", hidden_case},
	{"personality", personality_case},
	{"shm", shm_case},
	{"fd_list", fd_list_case},
};

enum { CASE_COUNT = sizeof(CASES) / sizeof(CASES[0]) };

// The case that --restart adds, which is not in the table: it ends the
// process that answers the others' calls, so it runs last.
static const struct test_case RESTART = {"restart", restart_case};

// Runs case `test` and says how it went; returns whether it passed.
static int run_case(const struct test_case *test) {
	case_name = test->name;
	case_failures = 0;
	case_skipped = 0;
	test->run();
	if(case_failures) {
		printf("view: %s FAILED\n", test->name);
		fflush(stdout);
		return 0;
	}
	if(!case_skipped)
		printf("view: %s ok\n", test->name);
	fflush(stdout);
	return 1;
}

// Finds this program's own paths from `argv0`, which is a path as the shell
// that ran it wrote it: the one it was started by, made absolute, and the
// real one. Returns whether it is one.
static int find_self(const char *argv0) {
	if(!strchr(argv0, '/'))
		return 0;
	int length;
	if(argv0[0] == '/') {
		length = snprintf(given_path, sizeof(given_path), "%s", argv0);
	} else {
		char folder[PATH_MAX];
		if(!getcwd(folder, sizeof(folder)))
			return 0;
		length = snprintf(given_path, sizeof(given_path), "%s/%s", folder, argv0);
	}
	return length < (int)sizeof(given_path) && realpath(argv0, real_path) != NULL;
}

// Runs the cases the arguments name, or all of them, and ends with how many
// passed, skipped or failed.
int main(int argc, char **argv) {
	setvbuf(stdout, NULL, _IOLBF, 0);
	if(!find_self(argv[0])) {
		printf("view: run it by its path, which the cases compare with the system's\n");
		return 2;
	}
	int selected[CASE_COUNT] = {0};
	int some_selected = 0, restart = 0;
	for(int i = 1; i < argc; i++) {
		if(!strcmp(argv[i], "--restart")) {
			restart = 1;
		} else if(i + 1 < argc && !strcmp(argv[i], "--linuxd")) {
			linuxd_number = (pid_t)atoi(argv[++i]);
		} else {
			int found = 0;
			for(int j = 0; j < CASE_COUNT; j++)
				if(!strcmp(argv[i], CASES[j].name))
					selected[j] = found = some_selected = 1;
			if(!found) {
				printf("view: there is no case named %s\n", argv[i]);
				return 2;
			}
		}
	}
	struct utsname name;
	if(uname(&name) == 0)
		on_dukovo = strstr(name.version, "Dukovo") != NULL;
	printf("view: running on %s\n", on_dukovo ? "Dukovo" : "Linux");
	int ran = 0, failed = 0, skipped = 0;
	for(int i = 0; i < CASE_COUNT; i++) {
		if(some_selected && !selected[i])
			continue;
		ran++;
		if(!run_case(&CASES[i]))
			failed++;
		skipped += case_skipped;
	}
	if(restart) {
		ran++;
		if(!run_case(&RESTART))
			failed++;
		skipped += case_skipped;
	}
	if(failed) {
		printf("view: %d of %d cases failed\n", failed, ran);
		return 1;
	}
	if(skipped)
		printf("view: all %d cases passed on %s, %d skipped\n", ran,
				on_dukovo ? "Dukovo" : "Linux", skipped);
	else
		printf("view: all %d cases passed on %s\n", ran, on_dukovo ? "Dukovo" : "Linux");
	return 0;
}
