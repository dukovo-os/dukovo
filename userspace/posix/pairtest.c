// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The network-pair boot test's program (ADRs 0331 to 0335): two machines
// on a cable. Machine a, `pairtest a`, joins its card on QEMU's network,
// net0, and the cable, net1, in the bridge br0, with VLAN 7 on the
// bridge, br0.7, at 192.168.7.1 and fd07::1; it serves a page on port
// 8080 and waits for machine b on port 7000. Machine b, `pairtest b`,
// has the other end of the cable, net0, and gets its addresses through
// a's bridge from QEMU's DHCP server and routers, IPv6 by SLAAC, with
// VLAN 7 on its card, net0.7, at 192.168.7.2 and fd07::2. It connects
// to a and does what a asks, a line each:
//
// - `curl URL`: fetches URL with curl(1), and says `ok` and the page's
//   first line, or `failed` and why;
// - `probe ADDRESS PORT`: connects there, and says `open`, `refused`,
//   or `blocked` when nothing answers within PROBE_MS;
// - `done`: says `bye` and ends.
//
// a has b fetch its page over VLAN 7 by IPv6 and IPv4, and over the
// bridge at a's DHCP address and SLAAC one, then checks the firewall
// from the other machine: a program run as a person runs it, without
// the inbound right, listens for this machine alone until `firewall
// allow` names it; a rule denies a port to one address; and port 22
// opens only while the port22 service, sshd's stand-in, runs.
// `pairtest serve PORT` is that service: it listens on PORT.

#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <radix/radix.h>
#include <sys/socket.h>

#include "check.h"

// The port of a's page, of the program a runs as a person would, and of
// a's control connection, which b makes.
#define PAGE_PORT 8080
#define USER_PORT 8081
#define CONTROL_PORT 7000
// How long a probe waits for an answer before it calls the port blocked:
// what the firewall drops gets none.
#define PROBE_MS 5000
// a's page, a line.
#define PAGE "pairtest: a page from machine a"
_Static_assert(sizeof(PAGE "\n") - 1 == 32, "the page's Content-Length is 32");

// Runs `command` with sh(1), saying so: its exit code, or -1.
static int run(const char *command) {
	printf("pairtest: $ %s\n", command);
	fflush(stdout);
	int status = system(command);
	return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

// Runs `command` with sh(1) and keeps what it writes, its standard error
// too, in `buffer`: its exit code, or -1.
static int output(const char *command, char *buffer, size_t size) {
	char line[512];
	snprintf(line, sizeof(line), "%s 2>&1", command);
	FILE *pipe = popen(line, "r");
	if(!pipe) {
		buffer[0] = 0;
		return -1;
	}
	size_t length = fread(buffer, 1, size - 1, pipe);
	buffer[length] = 0;
	int status = pclose(pipe);
	return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

// Runs `command` until what it writes holds `needle`, for at most
// `seconds`: whether it did.
static int wait_for(const char *command, const char *needle, int seconds) {
	static char buffer[8192];
	long long deadline = check_now_ns() + seconds * 1000000000LL;
	while(check_now_ns() < deadline) {
		if(output(command, buffer, sizeof(buffer)) == 0 && strstr(buffer, needle))
			return 1;
		check_sleep_ms(500);
	}
	printf("pairtest: `%s` never said \"%s\"; it said: %s\n", command, needle, buffer);
	return 0;
}

// Writes `text` into `path`.
static void write_file(const char *path, const char *text) {
	FILE *file = fopen(path, "w");
	CHECK(file);
	if(!file)
		return;
	CHECK(fputs(text, file) >= 0);
	CHECK(fclose(file) == 0);
}

// A TCP socket listening on every address, IPv4 and IPv6, at `port`.
static int listening(unsigned short port) {
	int fd = socket(AF_INET6, SOCK_STREAM, 0);
	CHECK(fd >= 0);
	int yes = 1;
	setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));
	struct sockaddr_in6 at;
	memset(&at, 0, sizeof(at));
	at.sin6_family = AF_INET6;
	at.sin6_port = htons(port);
	at.sin6_addr = in6addr_any;
	CHECK(bind(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	CHECK(listen(fd, 8) == 0);
	return fd;
}

// The address `text` at `port`, IPv4 or IPv6, in `out`: its length, or
// 0 if it is neither.
static socklen_t address(const char *text, unsigned short port, struct sockaddr_storage *out) {
	memset(out, 0, sizeof(*out));
	struct sockaddr_in6 *six = (struct sockaddr_in6 *)out;
	struct sockaddr_in *four = (struct sockaddr_in *)out;
	if(inet_pton(AF_INET6, text, &six->sin6_addr) == 1) {
		six->sin6_family = AF_INET6;
		six->sin6_port = htons(port);
		return sizeof(*six);
	}
	if(inet_pton(AF_INET, text, &four->sin_addr) == 1) {
		four->sin_family = AF_INET;
		four->sin_port = htons(port);
		return sizeof(*four);
	}
	return 0;
}

// Connects to `text` at `port`, waiting at most `ms`: the socket, or -1
// with `what` saying why: "refused", "blocked" or "error".
static int connect_to(const char *text, unsigned short port, int ms, const char **what) {
	struct sockaddr_storage to;
	socklen_t length = address(text, port, &to);
	*what = "error";
	if(!length)
		return -1;
	int fd = socket(to.ss_family, SOCK_STREAM | SOCK_NONBLOCK, 0);
	if(fd < 0)
		return -1;
	if(connect(fd, (struct sockaddr *)&to, length) < 0 && errno != EINPROGRESS) {
		*what = errno == ECONNREFUSED ? "refused" : "error";
		close(fd);
		return -1;
	}
	struct pollfd p = {fd, POLLOUT, 0};
	if(poll(&p, 1, ms) == 0) {
		*what = "blocked";
		close(fd);
		return -1;
	}
	int error = 0;
	socklen_t size = sizeof(error);
	getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &size);
	if(error) {
		*what = error == ECONNREFUSED ? "refused" : "error";
		close(fd);
		return -1;
	}
	fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) & ~O_NONBLOCK);
	*what = "open";
	return fd;
}

// Answers each connection to `server` with `text`, until killed.
static void answer(int server, const char *text) {
	for(;;) {
		int fd = accept(server, NULL, NULL);
		if(fd < 0) {
			if(errno == EINTR)
				continue;
			_exit(1);
		}
		// What it was asked: an HTTP request's head, or nothing.
		struct pollfd p = {fd, POLLIN, 0};
		char request[1024];
		size_t have = 0;
		while(have < sizeof(request) - 1 && poll(&p, 1, 300) == 1) {
			ssize_t got = read(fd, request + have, sizeof(request) - 1 - have);
			if(got <= 0)
				break;
			have += (size_t)got;
			request[have] = 0;
			if(strstr(request, "\r\n\r\n"))
				break;
		}
		(void)!write(fd, text, strlen(text));
		// Then until the other end closes, so that nothing it sent is
		// left unread, which would end the connection with a reset.
		shutdown(fd, SHUT_WR);
		while(poll(&p, 1, 2000) == 1 && read(fd, request, sizeof(request)) > 0) {
		}
		close(fd);
	}
}

// Starts a child that answers each connection to `server` with `text`.
static pid_t answering(int server, const char *text) {
	pid_t pid = check_fork();
	if(pid == 0)
		answer(server, text);
	close(server);
	return pid;
}

// Ends the child `pid`.
static void end(pid_t pid) {
	kill(pid, SIGKILL);
	check_wait(pid);
}

// `pairtest serve PORT`, the port22 service: answers each connection to
// PORT with a line, until Spark stops it.
static int serve(const char *port) {
	int server = listening((unsigned short)atoi(port));
	char line[64];
	snprintf(line, sizeof(line), "pairtest: port %s\n", port);
	answer(server, line);
	return 0;
}

// Machine a's connection to machine b, a stream each way: one stdio
// stream may not go from reading to writing without a seek, which a
// socket has not.
static FILE *control_in, *control_out;

// Asks b to do `command`: whether it answered `wanted` (a prefix).
static int ask(const char *command, const char *wanted) {
	printf("pairtest: asking b: %s\n", command);
	fflush(stdout);
	fprintf(control_out, "%s\n", command);
	fflush(control_out);
	char line[512];
	if(!fgets(line, sizeof(line), control_in)) {
		printf("pairtest: FAILED: b did not answer `%s`\n", command);
		check_failures++;
		return 0;
	}
	line[strcspn(line, "\n")] = 0;
	printf("pairtest: b: %s\n", line);
	if(strncmp(line, wanted, strlen(wanted)) != 0) {
		printf("pairtest: FAILED: b answered `%s` with `%s`, not `%s`\n", command, line, wanted);
		check_failures++;
		return 0;
	}
	return 1;
}

// The address on `line` after `prefix` (such as "inet6 fec0:"), up to
// its prefix length, in `out`.
static int address_after(const char *text, const char *prefix, const char *also, char *out,
		size_t size) {
	for(const char *at = strstr(text, prefix); at; at = strstr(at + 1, prefix)) {
		const char *end = strchr(at, '\n');
		size_t length = end ? (size_t)(end - at) : strlen(at);
		char line[256];
		snprintf(line, sizeof(line), "%.*s", (int)length, at);
		if(also && !strstr(line, also))
			continue;
		const char *start = strchr(line, ' ') + 1;
		size_t cut = strcspn(start, "/ ");
		if(cut >= size)
			return 0;
		memcpy(out, start, cut);
		out[cut] = 0;
		return 1;
	}
	return 0;
}

// Machine a's bridge and VLAN.
static void bridge(void) {
	// netd's settings for the VLAN first, so that it never asks DHCP.
	write_file("/system/settings/network",
			"[br0.7]\ndhcp = no\naddress = 192.168.7.1/24\naddress = fd07::1/64\n");
	CHECK(run("service netd reload") == 0);
	CHECK(run("ip link add br0 type bridge") == 0);
	CHECK(run("ip link set net0 master br0") == 0);
	CHECK(run("ip link set net1 master br0") == 0);
	CHECK(run("ip link add link br0 name br0.7 type vlan id 7") == 0);
	CHECK(wait_for("ip -4 address show dev br0", "inet 10.0.2.", 120));
	CHECK(wait_for("ip -6 address show dev br0", "inet6 fec0:", 120));
	CHECK(wait_for("ip -6 address show dev br0.7", "inet6 fd07::1/64 scope global \n", 60));
	CHECK(run("ip -d link show") == 0);
	CHECK(run("ip address show") == 0);
}

// The program a runs as a person would: a child with a network
// capability that lacks the inbound right, listening on USER_PORT.
static pid_t as_a_person(void) {
	int ready[2];
	CHECK(pipe(ready) == 0);
	pid_t pid = check_fork();
	if(pid == 0) {
		close(ready[0]);
		struct radix_capability_info_results info;
		CHECK(radix_capability_info(RADIX_STARTUP_SLOT_NETWORK, &info) == 0);
		uint32_t slot = RADIX_STARTUP_SLOT_NETWORK + RADIX_STARTUP_NETWORK_SLOTS - 1;
		radix_capability_delete(slot);
		CHECK(radix_network_restrict(RADIX_STARTUP_SLOT_NETWORK, slot, 0, 65536,
					  info.rights & ~(uint64_t)RADIX_RIGHTS_INBOUND)
				== 0);
		CHECK(radix_capability_delete(RADIX_STARTUP_SLOT_NETWORK) == 0);
		CHECK(radix_capability_move_to(slot, RADIX_STARTUP_SLOT_NETWORK) == 0);
		int server = listening(USER_PORT);
		(void)!write(ready[1], "!", 1);
		close(ready[1]);
		answer(server, "pairtest: a person's program\n");
	}
	close(ready[1]);
	char byte;
	CHECK(read(ready[0], &byte, 1) == 1);
	close(ready[0]);
	return pid;
}

// Whether this machine reaches its own `port` at `text`: the firewall
// leaves what it sends itself alone.
static int reaches_itself(const char *text, unsigned short port) {
	const char *what;
	int fd = connect_to(text, port, PROBE_MS, &what);
	if(fd < 0) {
		printf("pairtest: connecting to itself at %s port %u: %s\n", text, port, what);
		return 0;
	}
	char buffer[128];
	ssize_t got = read(fd, buffer, sizeof(buffer) - 1);
	close(fd);
	return got > 0;
}

// Waits at most 30 seconds for `port` on ::1 to be `open` or not.
static int local_port(unsigned short port, int open) {
	for(int i = 0; i < 60; i++) {
		const char *what;
		int fd = connect_to("::1", port, 1000, &what);
		if(fd >= 0)
			close(fd);
		if((fd >= 0) == open)
			return 1;
		check_sleep_ms(500);
	}
	return 0;
}

static void pages(void) {
	// Over VLAN 7, by IPv6 and IPv4.
	CHECK(ask("curl http://[fd07::1]:8080/", "ok " PAGE));
	CHECK(ask("curl http://192.168.7.1:8080/", "ok " PAGE));
	// Over the bridge, at the addresses a got through it.
	static char shown[8192];
	char four[64], six[64], command[160];
	CHECK(output("ip -4 address show dev br0", shown, sizeof(shown)) == 0);
	CHECK(address_after(shown, "inet 10.0.2.", NULL, four, sizeof(four)));
	snprintf(command, sizeof(command), "curl http://%s:8080/", four);
	CHECK(ask(command, "ok " PAGE));
	CHECK(output("ip -6 address show dev br0", shown, sizeof(shown)) == 0);
	CHECK(address_after(shown, "inet6 fec0:", "stable-privacy", six, sizeof(six)));
	snprintf(command, sizeof(command), "curl http://[%s]:8080/", six);
	CHECK(ask(command, "ok " PAGE));
	printf("pairtest: b fetched a's page over VLAN 7 and the bridge, by IPv6 and IPv4\n");
}

static void a_person(void) {
	pid_t pid = as_a_person();
	// Other machines do not reach it; this one does.
	CHECK(ask("probe fd07::1 8081", "blocked"));
	CHECK(reaches_itself("fd07::1", USER_PORT));
	static char status[8192];
	CHECK(output("firewall status", status, sizeof(status)) == 0);
	printf("%s", status);
	CHECK(strstr(status, "this machine only"));
	CHECK(strstr(status, "To allow it from other machines: firewall allow program "
						 "/system/tests/posix/pairtest"));
	// A rule for its program lets them in, and only to its port.
	CHECK(run("firewall allow tcp 8081 program /system/tests/posix/pairtest") == 0);
	CHECK(ask("probe fd07::1 8081", "open"));
	CHECK(ask("probe 192.168.7.1 8081", "open"));
	CHECK(output("firewall status", status, sizeof(status)) == 0);
	printf("%s", status);
	CHECK(strstr(status, "other machines (a rule)"));
	CHECK(run("firewall list") == 0);
	CHECK(run("firewall remove 1") == 0);
	CHECK(ask("probe fd07::1 8081", "blocked"));
	end(pid);
	printf("pairtest: a person's program was for this machine alone until a rule allowed it\n");
}

static void deny(void) {
	// A rule decides before the listener's inbound right.
	CHECK(run("firewall deny tcp 8080 from fd07::2") == 0);
	CHECK(ask("probe fd07::1 8080", "blocked"));
	CHECK(ask("probe 192.168.7.1 8080", "open"));
	CHECK(run("firewall remove 1") == 0);
	CHECK(ask("probe fd07::1 8080", "open"));
	printf("pairtest: a rule denied port 8080 to fd07::2 alone\n");
}

static void port22(void) {
	CHECK(ask("probe fd07::1 22", "blocked"));
	CHECK(run("service port22 start") == 0);
	CHECK(local_port(22, 1));
	CHECK(ask("probe fd07::1 22", "open"));
	CHECK(ask("probe 192.168.7.1 22", "open"));
	CHECK(ask("probe fd07::1 23", "blocked"));
	CHECK(run("service port22 stop") == 0);
	CHECK(local_port(22, 0));
	CHECK(ask("probe fd07::1 22", "blocked"));
	printf("pairtest: port 22 was open only while its service ran\n");
}

static int a(void) {
	RUN(bridge);
	pid_t page = answering(listening(PAGE_PORT), "HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\n"
											   "Content-Length: 32\r\n\r\n" PAGE "\n");
	int server = listening(CONTROL_PORT);
	printf("pairtest: waiting for b on port %d\n", CONTROL_PORT);
	fflush(stdout);
	struct pollfd p = {server, POLLIN, 0};
	CHECK(poll(&p, 1, 300000) == 1);
	int fd = accept(server, NULL, NULL);
	CHECK(fd >= 0);
	close(server);
	if(fd >= 0) {
		control_in = fdopen(fd, "r");
		control_out = fdopen(dup(fd), "w");
		RUN(pages);
		RUN(a_person);
		RUN(deny);
		RUN(port22);
		CHECK(ask("done", "bye"));
		fclose(control_out);
		fclose(control_in);
	}
	end(page);
	return check_end();
}

// Does what a asks, from `from`, and answers it on `to`.
static void follow(FILE *from, FILE *to) {
	char line[512];
	while(fgets(line, sizeof(line), from)) {
		line[strcspn(line, "\n")] = 0;
		printf("pairtest: a asks: %s\n", line);
		char reply[1100];
		char host[128];
		unsigned port;
		if(!strncmp(line, "curl ", 5)) {
			char command[600], page[512];
			snprintf(command, sizeof(command), "curl -gsS --max-time 30 '%s'", line + 5);
			int code = output(command, page, sizeof(page));
			page[strcspn(page, "\n")] = 0;
			snprintf(reply, sizeof(reply), "%s %s", code == 0 ? "ok" : "failed", page);
		} else if(sscanf(line, "probe %127s %u", host, &port) == 2) {
			const char *what;
			int fd = connect_to(host, (unsigned short)port, PROBE_MS, &what);
			if(fd >= 0)
				close(fd);
			snprintf(reply, sizeof(reply), "%s", what);
		} else if(!strcmp(line, "done")) {
			fprintf(to, "bye\n");
			fflush(to);
			return;
		} else {
			snprintf(reply, sizeof(reply), "what is `%s`?", line);
		}
		printf("pairtest: answering: %s\n", reply);
		fprintf(to, "%s\n", reply);
		fflush(to);
	}
	printf("pairtest: FAILED: a went before it was done\n");
	check_failures++;
}

static int b(void) {
	// QEMU's router sets neither M nor O in its adverts, but answers
	// DHCPv6 for the settings.
	write_file("/system/settings/network",
			"[net0]\ndhcpv6 = settings\n"
			"[net0.7]\ndhcp = no\naddress = 192.168.7.2/24\naddress = fd07::2/64\n");
	CHECK(run("service netd reload") == 0);
	CHECK(run("ip link add link net0 name net0.7 type vlan id 7") == 0);
	// Its addresses, through a's bridge.
	CHECK(wait_for("ip -4 address show dev net0", "inet 10.0.2.", 240));
	CHECK(wait_for("ip -6 address show dev net0", "inet6 fec0:", 240));
	CHECK(wait_for("ip -6 address show dev net0.7", "inet6 fd07::2/64 scope global \n", 60));
	CHECK(run("ip address show") == 0);
	// a may take longer to boot and set up.
	int fd = -1;
	const char *what = "error";
	for(int i = 0; i < 300 && fd < 0; i++) {
		fd = connect_to("fd07::1", CONTROL_PORT, 1000, &what);
		if(fd < 0)
			check_sleep_ms(500);
	}
	if(fd < 0)
		printf("pairtest: FAILED: cannot reach a on port %d: %s\n", CONTROL_PORT, what);
	CHECK(fd >= 0);
	if(fd >= 0) {
		printf("pairtest: connected to a\n");
		FILE *from = fdopen(fd, "r");
		FILE *to = fdopen(dup(fd), "w");
		follow(from, to);
		fclose(to);
		fclose(from);
	}
	return check_end();
}

int main(int argc, char **argv) {
	check_start("pairtest");
	if(argc == 3 && !strcmp(argv[1], "serve"))
		return serve(argv[2]);
	if(argc == 2 && !strcmp(argv[1], "a"))
		return a();
	if(argc == 2 && !strcmp(argv[1], "b"))
		return b();
	printf("usage: pairtest a | b | serve PORT\n");
	return 2;
}
