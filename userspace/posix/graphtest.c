// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the service-graph boot test runs (ADR 0470, ADR 0471): services
// that name each other in requires and after, say when they are ready
// and check their health. graphtest writes their files, starts them with
// service(8) and checks what `service NAME status` says: a service waits
// for one it requires until that one listens; one whose requirement ends
// is stopped, as is one whose requirement a request stops; one that
// keeps failing its health check is restarted until Spark gives up on
// it, and starts again once fixed; two that wait for each other never
// start; and a setup step is done before what requires it starts.
//
// It checks how service(8) waits, too: a start shows Spark's lines about
// the service and what it requires as they come and returns once the
// service is ready, or fails saying why; a stop names what it stops with
// the service; four waits run at once and a fifth start is answered
// without one; and a wait that Ctrl-C ends leaves its start going. Then
// it powers the machine off with g-web and g-db running, for the test's
// script to see g-web stop first.
//
// As a service, it runs in a mode: `--listen PORT DELAY_MS` listens on
// PORT after DELAY_MS and sleeps; `--sleep` sleeps; `--exit CODE` exits
// with CODE; `--check FILE`, a health check, passes when FILE is there.

#include <netinet/in.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"
#include "programs.h"

#define SERVICE "/system/commands/service"
#define POWEROFF "/system/commands/poweroff"
#define DIR "/system/settings/spark/services"
#define SELF "/system/tests/posix/graphtest"

// The file g-sick's health check looks for, which every service's view
// holds (/system/settings is in each, read-only).
#define SICK_OK "/system/settings/g-sick-ok"

// A database that listens on 7001 1.5 s after it starts, and a web
// server that requires it.
static const char G_DB[] = "service_version = 2\n"
		"description = \"Listens after a while\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--listen\", \"7001\", \"1500\"]\n"
		"enabled = false\n"
		"restart = \"on-failure\"\n"
		"\n"
		"[ready]\n"
		"when = \"listening\"\n"
		"port = 7001\n"
		"timeout = 20\n"
		"\n"
		"[permissions]\n"
		"network.listen = [7001]\n";

static const char G_WEB[] = "service_version = 2\n"
		"description = \"Requires g-db\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--sleep\"]\n"
		"enabled = false\n"
		"requires = [\"g-db\"]\n";

// A service that fails, and one that requires it.
static const char G_BROKEN[] = "service_version = 2\n"
		"description = \"Fails at once\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--exit\", \"1\"]\n"
		"enabled = false\n"
		"restart = \"never\"\n";

static const char G_NEEDS[] = "service_version = 2\n"
		"description = \"Requires g-broken\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--sleep\"]\n"
		"enabled = false\n"
		"requires = [\"g-broken\"]\n";

// A service whose health check fails until SICK_OK is there.
static const char G_SICK[] = "service_version = 2\n"
		"description = \"Fails its health check\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--sleep\"]\n"
		"enabled = false\n"
		"restart = \"on-failure\"\n"
		"give_up_after = 2\n"
		"\n"
		"[health]\n"
		"command = [\"" SELF "\", \"--check\", \"" SICK_OK "\"]\n"
		"every = 1\n"
		"timeout = 1\n"
		"failures = 2\n";

// Two services that wait for each other.
static const char G_A[] = "service_version = 2\n"
		"description = \"Waits for g-b\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--sleep\"]\n"
		"enabled = false\n"
		"after = [\"g-b\"]\n";

static const char G_B[] = "service_version = 2\n"
		"description = \"Waits for g-a\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--sleep\"]\n"
		"enabled = false\n"
		"after = [\"g-a\"]\n";

// A setup step, and a service that requires it.
static const char G_SETUP[] = "service_version = 2\n"
		"description = \"Sets up and ends\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--exit\", \"0\"]\n"
		"enabled = false\n"
		"restart = \"never\"\n"
		"\n"
		"[ready]\n"
		"when = \"ended\"\n";

static const char G_APP[] = "service_version = 2\n"
		"description = \"Requires g-setup\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--sleep\"]\n"
		"enabled = false\n"
		"requires = [\"g-setup\"]\n";

// A service, and one that requires it, for a stop request.
static const char G_BASE[] = "service_version = 2\n"
		"description = \"Stopped by a request\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--sleep\"]\n"
		"enabled = false\n";

static const char G_TOP[] = "service_version = 2\n"
		"description = \"Requires g-base\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--sleep\"]\n"
		"enabled = false\n"
		"requires = [\"g-base\"]\n";

// A service that just runs, for the watch counts.
static const char G_PLAIN[] = "service_version = 2\n"
		"description = \"Runs\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--sleep\"]\n"
		"enabled = false\n";

// A service that listens 10 s after it starts, for the watch counts, with
// its port three times: in its arguments, [ready] and its permissions.
static const char G_SLOW[] = "service_version = 2\n"
		"description = \"Listens after 10 s\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--listen\", \"%d\", \"10000\"]\n"
		"enabled = false\n"
		"\n"
		"[ready]\n"
		"when = \"listening\"\n"
		"port = %d\n"
		"timeout = 60\n"
		"\n"
		"[permissions]\n"
		"network.listen = [%d]\n";

// Writes `text` to `path`.
static void write_file(const char *path, const char *text) {
	int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0644);
	CHECK(fd >= 0);
	if(fd < 0)
		return;
	CHECK(write(fd, text, strlen(text)) == (ssize_t)strlen(text));
	close(fd);
}

// Writes service `name`'s file.
static void write_service(const char *name, const char *text) {
	char path[128];
	snprintf(path, sizeof(path), DIR "/%s.toml", name);
	write_file(path, text);
}

// Waits up to `seconds` for `service NAME status` to say `text`; says
// what it said last if it never does.
static int says(const char *name, const char *text, int seconds) {
	long long deadline = check_now_ns() + seconds * 1000000000LL;
	for(;;) {
		run_program((const char *const[]){SERVICE, name, "status", NULL}, "", 0);
		if(strstr(output, text))
			return 1;
		if(check_now_ns() >= deadline) {
			printf("graphtest: %s's status never said \"%s\"; it said:\n%s", name, text, output);
			return 0;
		}
		check_sleep_ms(200);
	}
}

// A `service NAME start` that waits, run in the background with its
// output and errors on `from`.
struct waiting {
	pid_t pid;
	int from;
};

// Runs `service NAME start` in the background.
static struct waiting start_waiting(const char *name) {
	struct waiting waiting = {-1, -1};
	int out[2];
	if(pipe(out))
		return waiting;
	waiting.pid = fork();
	if(!waiting.pid) {
		dup2(out[1], STDOUT_FILENO);
		dup2(out[1], STDERR_FILENO);
		close(out[0]);
		close(out[1]);
		execv(SERVICE, (char *const[]){SERVICE, (char *)name, "start", NULL});
		_exit(127);
	}
	close(out[1]);
	waiting.from = out[0];
	return waiting;
}

// Waits for `waiting` to end, with what it wrote in `output`; returns its
// exit status, or 128 and the signal that ended it.
static int finish(struct waiting waiting) {
	size_t len = 0;
	ssize_t got;
	while(len < sizeof(output) - 1 && (got = read(waiting.from, output + len, sizeof(output) - 1 - len)) > 0)
		len += got;
	output[len] = 0;
	close(waiting.from);
	int status;
	if(waitpid(waiting.pid, &status, 0) != waiting.pid)
		return -1;
	return WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
}

// g-web waits for g-db, which it requires, until g-db listens; a start
// that does not wait returns at once.
static void waits_until_ready(void) {
	write_service("g-db", G_DB);
	write_service("g-web", G_WEB);
	STATUS(0, SERVICE, "g-web", "start", "--no-wait");
	// g-db listens only after 1.5 s.
	CHECK(says("g-web", "Active: activating (waiting for g-db)", 2));
	CHECK(says("g-db", "Active: activating (starting as process", 2));
	CHECK(says("g-web", "Active: active (running)", 15));
	CHECK(says("g-db", "Active: active (running)", 1));
	CHECK(says("g-web", "Needs: requires g-db", 1));
	CHECK(says("g-db", "Started: by g-web, which requires it", 1));
}

// A stop names what it stops with the service; a start waits until the
// service is ready, with Spark's lines about it and what it requires as
// they come, and says who asked.
static void a_start_shows_the_way(void) {
	CONTAINS("stopping g-web: it requires g-db, which was stopped", SERVICE, "g-db", "stop");
	CHECK(says("g-web", "Active: inactive (dead)", 1));
	long long asked = check_now_ns();
	CONTAINS("g-db is ready (listening on 7001)", SERVICE, "g-web", "start");
	// g-db listens only after 1.5 s, and the start waited for it.
	CHECK(check_now_ns() - asked >= 1500000000LL);
	CHECK(strstr(output, "started g-db (" SELF ") as process") != NULL);
	CHECK(strstr(output, "started g-web (" SELF ") as process") != NULL);
	CHECK(says("g-web", "Active: active (running)", 1));
	CHECK(says("g-web", "Asked: to start by ", 1));
	// The services' logs hold Spark's lines about them.
	CONTAINS("started g-web (" SELF ") as process", SERVICE, "g-web", "logs");
}

// g-needs stops once g-broken, which it requires, ends for good, and the
// start that waited for it fails saying why.
static void stops_with_its_requirement(void) {
	write_service("g-broken", G_BROKEN);
	write_service("g-needs", G_NEEDS);
	output_errors = 1;
	STATUS(1, SERVICE, "g-needs", "start");
	output_errors = 0;
	CHECK(strstr(output, "could not start g-needs: it requires g-broken, which is stopped") != NULL);
	CHECK(says("g-needs", "Why: it requires g-broken, which is stopped", 10));
	CHECK(says("g-needs", "Active: inactive (dead)", 10));
	CHECK(says("g-broken", "Last: exited with code 0x1", 1));
}

// Stopping g-base by a request stops g-top, which requires it.
static void a_stop_stops_what_requires_it(void) {
	write_service("g-base", G_BASE);
	write_service("g-top", G_TOP);
	STATUS(0, SERVICE, "g-top", "start");
	CHECK(says("g-top", "Active: active (running)", 5));
	CONTAINS("stopping g-top: it requires g-base, which was stopped", SERVICE, "g-base", "stop");
	CHECK(says("g-top", "Why: it requires g-base, which was stopped", 5));
	CHECK(says("g-top", "Active: inactive (dead)", 5));
	CHECK(says("g-base", "Active: inactive (dead)", 5));
}

// g-sick fails its health checks and is restarted until Spark gives up
// on it; fixed, a start brings it back.
static void gives_up_on_a_sick_service(void) {
	unlink(SICK_OK);
	write_service("g-sick", G_SICK);
	STATUS(0, SERVICE, "g-sick", "start");
	CHECK(says("g-sick", "Active: failed (Spark gave up on it)", 40));
	CHECK(says("g-sick", "Why: it failed to start 2 times in a row", 1));
	CHECK(says("g-sick", "gives up after 2 failed starts in a row", 1));
	CHECK(says("g-sick", "Health: failing, 2 checks in a row (exited with code 0x1: no " SICK_OK ")", 1));
	write_file(SICK_OK, "");
	STATUS(0, SERVICE, "g-sick", "start");
	CHECK(says("g-sick", "Health: passing", 15));
	CHECK(says("g-sick", "Active: active (running)", 1));
}

// g-a and g-b wait for each other, so neither starts.
static void a_circle_never_starts(void) {
	write_service("g-a", G_A);
	write_service("g-b", G_B);
	STATUS(1, SERVICE, "g-a", "start");
	CHECK(says("g-a", "Why: it waits for itself, through g-a and g-b", 2));
	STATUS(1, SERVICE, "g-b", "start");
	CHECK(says("g-b", "Active: inactive (dead)", 1));
}

// g-app starts once g-setup, which it requires, is done.
static void a_setup_step_comes_first(void) {
	write_service("g-setup", G_SETUP);
	write_service("g-app", G_APP);
	STATUS(0, SERVICE, "g-app", "start");
	CHECK(says("g-app", "Active: active (running)", 10));
	CHECK(says("g-setup", "Active: active (exited)", 1));
}

// Four waits run at once, and a fifth start is answered without one; a
// wait that Ctrl-C ends leaves its start going and frees its watch.
static void four_waits_at_once(void) {
	struct waiting waits[4];
	char names[4][16];
	for(int i = 0; i < 4; i++) {
		char text[sizeof(G_SLOW) + 16];
		snprintf(names[i], sizeof(names[i]), "g-slow%d", i);
		snprintf(text, sizeof(text), G_SLOW, 7010 + i, 7010 + i, 7010 + i);
		write_service(names[i], text);
		waits[i] = start_waiting(names[i]);
	}
	// Each is starting once Spark has its request, and is watched from then.
	for(int i = 0; i < 4; i++)
		CHECK(says(names[i], "Active: activating (starting as process", 5));
	write_service("g-fifth", G_PLAIN);
	CONTAINS("not watched: 4 watches already", SERVICE, "g-fifth", "start");
	kill(waits[0].pid, SIGINT);
	CHECK(finish(waits[0]) == 130);
	CHECK(strstr(output, "g-slow0 goes on starting; `service g-slow0 status` says how it goes") != NULL);
	write_service("g-sixth", G_PLAIN);
	CONTAINS("started g-sixth (" SELF ") as process", SERVICE, "g-sixth", "start");
	CHECK(strstr(output, "not watched") == NULL);
	for(int i = 1; i < 4; i++) {
		CHECK(finish(waits[i]) == 0);
		CHECK(strstr(output, "is ready (listening on 701") != NULL);
	}
	CHECK(says("g-slow0", "Active: active (running)", 10));
	CONTAINS("g-slow0", SERVICE, "list");
}

// `--listen PORT DELAY_MS`: listens on PORT after DELAY_MS, then sleeps.
static int listen_after(int port, long delay_ms) {
	check_sleep_ms(delay_ms);
	int fd = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
	struct sockaddr_in address = {
		.sin_family = AF_INET,
		.sin_port = htons(port),
		.sin_addr.s_addr = htonl(INADDR_ANY),
	};
	if(fd < 0 || bind(fd, (struct sockaddr *)&address, sizeof(address)) || listen(fd, 4)) {
		printf("cannot listen on %d: %s\n", port, strerror(errno));
		return 1;
	}
	printf("listening on %d\n", port);
	fflush(stdout);
	for(;;)
		pause();
}

int main(int argc, char **argv) {
	if(argc == 4 && !strcmp(argv[1], "--listen"))
		return listen_after(atoi(argv[2]), atol(argv[3]));
	if(argc == 2 && !strcmp(argv[1], "--sleep"))
		for(;;)
			pause();
	if(argc == 3 && !strcmp(argv[1], "--exit"))
		return atoi(argv[2]);
	if(argc == 3 && !strcmp(argv[1], "--check")) {
		if(access(argv[2], F_OK) == 0)
			return 0;
		printf("no %s\n", argv[2]);
		return 1;
	}
	check_start("graphtest");
	RUN(waits_until_ready);
	RUN(a_start_shows_the_way);
	RUN(stops_with_its_requirement);
	RUN(a_stop_stops_what_requires_it);
	RUN(gives_up_on_a_sick_service);
	RUN(a_circle_never_starts);
	RUN(a_setup_step_comes_first);
	RUN(four_waits_at_once);
	int status = check_end();
	fflush(stdout);
	// Shutting down stops g-web before g-db, which it requires.
	STATUS(0, POWEROFF);
	return status;
}
