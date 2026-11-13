// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the service-graph boot test runs (ADR 0470): services that name
// each other in requires and after, say when they are ready and check
// their health. graphtest writes their files, starts them with
// service(8) and checks what `service NAME status` says: a service waits
// for one it requires until that one listens; one whose requirement ends
// is stopped, as is one whose requirement a request stops; one that
// keeps failing its health check is restarted until Spark gives up on
// it, and starts again once fixed; two that wait for each other never
// start; and a setup step is done before what requires it starts. Then
// it powers the machine off with g-web and g-db running, for the test's
// script to see g-web stop first.
//
// As a service, it runs in a mode: `--listen PORT DELAY_MS` listens on
// PORT after DELAY_MS and sleeps; `--sleep` sleeps; `--exit CODE` exits
// with CODE; `--check FILE`, a health check, passes when FILE is there.

#include <netinet/in.h>
#include <sys/socket.h>
#include <sys/stat.h>
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

// g-web waits for g-db, which it requires, until g-db listens.
static void waits_until_ready(void) {
	write_service("g-db", G_DB);
	write_service("g-web", G_WEB);
	STATUS(0, SERVICE, "g-web", "start");
	// g-db listens only after 1.5 s.
	CHECK(says("g-web", "Active: activating (waiting for g-db)", 2));
	CHECK(says("g-db", "Active: activating (starting as process", 2));
	CHECK(says("g-web", "Active: active (running)", 15));
	CHECK(says("g-db", "Active: active (running)", 1));
	CHECK(says("g-web", "Needs: requires g-db", 1));
	CHECK(says("g-db", "Started: by g-web, which requires it", 1));
}

// g-needs stops once g-broken, which it requires, ends for good.
static void stops_with_its_requirement(void) {
	write_service("g-broken", G_BROKEN);
	write_service("g-needs", G_NEEDS);
	STATUS(0, SERVICE, "g-needs", "start");
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
	STATUS(0, SERVICE, "g-base", "stop");
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
	RUN(stops_with_its_requirement);
	RUN(a_stop_stops_what_requires_it);
	RUN(gives_up_on_a_sick_service);
	RUN(a_circle_never_starts);
	RUN(a_setup_step_comes_first);
	int status = check_end();
	fflush(stdout);
	// Shutting down stops g-web before g-db, which it requires.
	STATUS(0, POWEROFF);
	return status;
}
