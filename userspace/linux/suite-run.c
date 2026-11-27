// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The runner of the linux-suite run's cases (ADR 0628): LTP's system call
// tests, each a program of its own, run one at a time, on Linux and in
// Dukovo's Linux view, the same way on both. ob builds it with the build
// machine's C library, as it builds the tests; on Dukovo it runs under
// `linux run`, which starts a program with a clean environment, so the
// runner gives each case the environment LTP's tests look for itself.
//
//   suite-run LIST ROOT OUT [SECONDS]
//
// LIST holds a case a line, as LTP's runtest files do: its name, then its
// command, a program in ROOT/testcases/bin and its arguments; a line that
// starts with # is a comment. Each case runs in a process group of its
// own, in /tmp, with ROOT/testcases/bin first in PATH, ROOT as LTPROOT,
// LTP's fuzzy-sync and runtime-bound tests shortened to a tenth
// (LTP_RUNTIME_MUL), its output in OUT/logs/NAME.log and its input
// /dev/null. A case still running after SECONDS (180 if none is given) is
// killed with its group, and once a case ends, whatever it left running in
// its group is killed too.
//
// For each case the runner appends a line to OUT/results.txt, `NAME STATUS
// MILLISECONDS`: the exit status (0 when the test passed; LTP adds 1 for a
// failure, 2 for a broken test, 4 for a warning and 32 for a test that
// does not apply), `signal-N` when a signal ended it, `timeout`, or
// `not-run` when it could not be started, and how long it ran. Three cases
// in a row that run out of time say the system no longer runs any, and the
// runner stops there rather than wait out the rest. It ends by saying, on
// its standard output, `suite-run: N cases, P passed`, and exits with 0,
// or with 2 when it could not read its list or write its results.

#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

enum {
	// The words of a case's line, at most.
	WORDS = 64,
	// The longest line, and the longest path given.
	LINE = 4096,
	// The longest path made of a given one and a name.
	PATH_LONGEST = 2 * LINE + 64,
	// What a case that could not be started exits with.
	NOT_RUN = 127,
	// How many cases in a row may run out of time before the runner stops.
	TIMEOUTS_IN_A_ROW = 3,
};

static long long now_ms(void) {
	struct timespec now;
	clock_gettime(CLOCK_MONOTONIC, &now);
	return (long long)now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

// Waits for `child` until `deadline` (now_ms's clock); returns 1 with its
// status in `status`, or 0 when it outlasted the deadline.
static int wait_until(pid_t child, long long deadline, int *status) {
	struct timespec step = {0, 20 * 1000 * 1000};
	for(;;) {
		pid_t done = waitpid(child, status, WNOHANG);
		if(done == child)
			return 1;
		if(done < 0 && errno != EINTR) {
			*status = NOT_RUN << 8;
			return 1;
		}
		if(now_ms() >= deadline)
			return 0;
		nanosleep(&step, NULL);
	}
}

// The environment of every case.
static char *environment[8];

static void make_environment(const char *root) {
	static char path[PATH_LONGEST], ltproot[PATH_LONGEST];
	snprintf(path, sizeof(path), "PATH=%s/testcases/bin:/usr/local/bin:/usr/bin:/bin", root);
	snprintf(ltproot, sizeof(ltproot), "LTPROOT=%s", root);
	char *entries[] = {path, ltproot, "TMPDIR=/tmp", "HOME=/tmp", "LANG=C",
			"LTP_COLORIZE_OUTPUT=0", "LTP_RUNTIME_MUL=0.1", NULL};
	memcpy(environment, entries, sizeof(entries));
}

// Starts the case whose command is `words` in a group of its own, with its
// output in `log`; returns its process, or -1.
static pid_t start(char **words, const char *root, const char *log) {
	pid_t child = fork();
	if(child != 0)
		return child;
	setpgid(0, 0);
	int in = open("/dev/null", O_RDONLY);
	int out = open(log, O_WRONLY | O_CREAT | O_TRUNC, 0644);
	if(in < 0 || out < 0 || dup2(in, 0) < 0 || dup2(out, 1) < 0 || dup2(out, 2) < 0)
		_exit(NOT_RUN);
	close(in);
	close(out);
	char program[PATH_LONGEST];
	snprintf(program, sizeof(program), "%s/testcases/bin/%s", root, words[0]);
	if(chdir("/tmp") != 0)
		_exit(NOT_RUN);
	execve(program, words, environment);
	dprintf(2, "suite-run: cannot run %s: %s\n", program, strerror(errno));
	_exit(NOT_RUN);
}

// What became of a case.
enum outcome { PASSED, FAILED, TIMED_OUT };

// Runs one case and writes its line of `results`; returns what became of
// it.
static enum outcome run_case(char **words, const char *root, const char *logs, int seconds,
		FILE *results) {
	char log[PATH_LONGEST], status_text[32];
	int log_fits = snprintf(log, sizeof(log), "%s/%s.log", logs, words[0]) < (int)sizeof(log);
	long long started = now_ms();
	pid_t child = log_fits ? start(words + 1, root, log) : -1;
	if(child < 0) {
		snprintf(status_text, sizeof(status_text), "not-run");
	} else {
		// The group, in case the child has not made it yet.
		setpgid(child, child);
		int status = 0;
		if(!wait_until(child, started + (long long)seconds * 1000, &status)) {
			kill(-child, SIGKILL);
			waitpid(child, &status, 0);
			snprintf(status_text, sizeof(status_text), "timeout");
		} else if(WIFSIGNALED(status)) {
			snprintf(status_text, sizeof(status_text), "signal-%d", WTERMSIG(status));
		} else if(WEXITSTATUS(status) == NOT_RUN) {
			snprintf(status_text, sizeof(status_text), "not-run");
		} else {
			snprintf(status_text, sizeof(status_text), "%d", WEXITSTATUS(status));
		}
		// What the case left running in its group.
		kill(-child, SIGKILL);
	}
	fprintf(results, "%s %s %lld\n", words[0], status_text, now_ms() - started);
	fflush(results);
	if(strcmp(status_text, "timeout") == 0)
		return TIMED_OUT;
	return strcmp(status_text, "0") == 0 ? PASSED : FAILED;
}

int main(int argc, char **argv) {
	if(argc < 4 || argc > 5) {
		fprintf(stderr, "usage: suite-run LIST ROOT OUT [SECONDS]\n");
		return 2;
	}
	const char *list = argv[1], *root = argv[2], *out = argv[3];
	int seconds = argc > 4 ? atoi(argv[4]) : 180;
	if(seconds <= 0) {
		fprintf(stderr, "suite-run: %s is not a number of seconds\n", argv[4]);
		return 2;
	}
	char logs[PATH_LONGEST], results_path[PATH_LONGEST];
	snprintf(logs, sizeof(logs), "%s/logs", out);
	snprintf(results_path, sizeof(results_path), "%s/results.txt", out);
	mkdir(out, 0755);
	mkdir(logs, 0755);
	FILE *cases = fopen(list, "r");
	if(!cases) {
		fprintf(stderr, "suite-run: %s: %s\n", list, strerror(errno));
		return 2;
	}
	FILE *results = fopen(results_path, "w");
	if(!results) {
		fprintf(stderr, "suite-run: %s: %s\n", results_path, strerror(errno));
		return 2;
	}
	make_environment(root);
	// A case that ends its group's readers leaves the runner be.
	signal(SIGPIPE, SIG_IGN);
	char line[LINE];
	int count = 0, passed = 0, timed_out = 0;
	while(timed_out < TIMEOUTS_IN_A_ROW && fgets(line, sizeof(line), cases)) {
		char *words[WORDS + 1];
		int n = 0;
		for(char *word = strtok(line, " \t\n"); word && n < WORDS; word = strtok(NULL, " \t\n"))
			words[n++] = word;
		if(n < 2 || words[0][0] == '#')
			continue;
		words[n] = NULL;
		enum outcome outcome = run_case(words, root, logs, seconds, results);
		passed += outcome == PASSED;
		timed_out = outcome == TIMED_OUT ? timed_out + 1 : 0;
		count++;
		// The children a case's processes left to the runner.
		while(waitpid(-1, NULL, WNOHANG) > 0) {
		}
	}
	if(timed_out == TIMEOUTS_IN_A_ROW)
		printf("suite-run: %d cases in a row ran out of time; the rest are not run\n",
				timed_out);
	printf("suite-run: %d cases, %d passed\n", count, passed);
	return 0;
}
