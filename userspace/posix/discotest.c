// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The network-discovery boot test's program (ADRs 0336 to 0338): two
// machines on a cable, each named for its letter with hostname(1), at
// 192.168.50.N and fd50::N on its card, net0, through network(5), and
// each running discoverd(8), which their service files leave off until
// the machine has its name. Machine b, `discotest b`, also answers
// WS-Discovery's probes as a Windows machine does, and connects to
// machine a, `discotest a`, which asks it, a line each:
//
// - `ping NAME`: pings NAME once, by IPv4, and says `ok` and what ping
//   said of the reply, or `failed`;
// - `done`: says `bye` and ends.
//
// a pings its own name, then b's by IPv4 and IPv6, has b ping a's, and
// lists with discover(1) what the cable has: b's workstation service by
// DNS-SD, and b as a WS-Discovery device. Then the local right: a ping
// socket reaches b, which is on a network this machine is on, with the
// local right and not with the send right alone (ADR 0337).

#include <arpa/inet.h>
#include <net/if.h>
#include <netinet/in.h>
#include <poll.h>
#include <radix/radix.h>
#include <sys/socket.h>

#include "check.h"

// a's control connection, which b makes.
#define CONTROL_PORT 7000
// WS-Discovery's group and port.
#define WSD_GROUP "239.255.255.250"
#define WSD_PORT 3702
// The endpoint b's WS-Discovery answers say it is.
#define ENDPOINT "urn:uuid:4f0b6a4e-5d1c-4e0e-9b39-646973636f62"

// Runs `command` with sh(1), saying so: its exit code, or -1.
static int run(const char *command) {
	printf("discotest: $ %s\n", command);
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

// Runs `command` until what it writes holds each of `needles`, for at
// most `seconds`: whether it did. It says what the command wrote last.
static int wait_for(const char *command, const char *const *needles, int seconds) {
	static char buffer[8192];
	long long deadline = check_now_ns() + seconds * 1000000000LL;
	while(check_now_ns() < deadline) {
		int all = output(command, buffer, sizeof(buffer)) == 0;
		for(const char *const *needle = needles; all && *needle; needle++)
			all = strstr(buffer, *needle) != NULL;
		if(all) {
			printf("discotest: `%s` said:\n%s", command, buffer);
			return 1;
		}
		check_sleep_ms(500);
	}
	printf("discotest: `%s` never said all it should; it said: %s\n", command, buffer);
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

// Names this machine `name`, puts it at 192.168.50.`n` and fd50::`n` on
// the cable, and starts discoverd once it has.
static void set_up(const char *name, int n) {
	char command[160], settings[160];
	snprintf(command, sizeof(command), "hostname %s", name);
	CHECK(run(command) == 0);
	snprintf(settings, sizeof(settings),
			"[net0]\ndhcp = no\ndhcpv6 = no\naddress = 192.168.50.%d/24\naddress = fd50::%d/64\n",
			n, n);
	write_file("/system/settings/network", settings);
	CHECK(run("service netd reload") == 0);
	char four[64], six[64];
	snprintf(four, sizeof(four), "inet 192.168.50.%d/24", n);
	snprintf(six, sizeof(six), "inet6 fd50::%d/64 scope global", n);
	const char *const addresses[] = {four, NULL};
	const char *const addresses6[] = {six, NULL};
	CHECK(wait_for("ip -4 address show dev net0", addresses, 60));
	CHECK(wait_for("ip -6 address show dev net0", addresses6, 60));
	CHECK(run("service discoverd start") == 0);
	const char *const answering[] = {"socket", NULL};
	CHECK(wait_for("ls /system/live/discoverd", answering, 30));
}

// The text of the element of `xml` whose local name is `name`, in `out`.
static int element(const char *xml, const char *name, char *out, size_t size) {
	char open[64];
	snprintf(open, sizeof(open), ":%s>", name);
	const char *at = strstr(xml, open);
	if(!at)
		return 0;
	at += strlen(open);
	size_t length = strcspn(at, "<");
	if(length >= size)
		return 0;
	memcpy(out, at, length);
	out[length] = 0;
	return 1;
}

// Answers WS-Discovery's probes to its group on net0, as a Windows
// machine does, from a child, until killed.
static pid_t answer_probes(void) {
	int fd = socket(AF_INET, SOCK_DGRAM, 0);
	CHECK(fd >= 0);
	int yes = 1;
	CHECK(setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes)) == 0);
	struct sockaddr_in at = {.sin_family = AF_INET, .sin_port = htons(WSD_PORT)};
	CHECK(bind(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	struct ip_mreqn group = {.imr_ifindex = (int)if_nametoindex("net0")};
	inet_pton(AF_INET, WSD_GROUP, &group.imr_multiaddr);
	CHECK(setsockopt(fd, IPPROTO_IP, IP_ADD_MEMBERSHIP, &group, sizeof(group)) == 0);
	pid_t pid = check_fork();
	if(pid)
		return pid;
	for(;;) {
		static char message[8192];
		struct sockaddr_in from;
		socklen_t length = sizeof(from);
		ssize_t got = recvfrom(fd, message, sizeof(message) - 1, 0, (struct sockaddr *)&from, &length);
		if(got <= 0)
			continue;
		message[got] = 0;
		char id[128];
		if(!strstr(message, "/discovery/Probe<") || !element(message, "MessageID", id, sizeof(id)))
			continue;
		printf("discotest: answering WS-Discovery's probe %s\n", id);
		fflush(stdout);
		static char reply[2048];
		int n = snprintf(reply, sizeof(reply),
				"<?xml version=\"1.0\" encoding=\"utf-8\"?>"
				"<soap:Envelope xmlns:soap=\"http://www.w3.org/2003/05/soap-envelope\" "
				"xmlns:wsa=\"http://schemas.xmlsoap.org/ws/2004/08/addressing\" "
				"xmlns:wsd=\"http://schemas.xmlsoap.org/ws/2005/04/discovery\" "
				"xmlns:wsdp=\"http://schemas.xmlsoap.org/ws/2006/02/devprof\" "
				"xmlns:pub=\"http://schemas.microsoft.com/windows/pub/2005/07\">"
				"<soap:Header>"
				"<wsa:To>http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous</wsa:To>"
				"<wsa:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/ProbeMatches</wsa:Action>"
				"<wsa:MessageID>urn:uuid:0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b</wsa:MessageID>"
				"<wsa:RelatesTo>%s</wsa:RelatesTo>"
				"</soap:Header>"
				"<soap:Body><wsd:ProbeMatches><wsd:ProbeMatch>"
				"<wsa:EndpointReference><wsa:Address>" ENDPOINT "</wsa:Address></wsa:EndpointReference>"
				"<wsd:Types>wsdp:Device pub:Computer</wsd:Types>"
				"<wsd:XAddrs>http://192.168.50.2:5357/" ENDPOINT "</wsd:XAddrs>"
				"<wsd:MetadataVersion>1</wsd:MetadataVersion>"
				"</wsd:ProbeMatch></wsd:ProbeMatches></soap:Body></soap:Envelope>",
				id);
		sendto(fd, reply, (size_t)n, 0, (struct sockaddr *)&from, length);
	}
}

// Machine a's connection to machine b, a stream each way.
static FILE *control_in, *control_out;

// Asks b to do `command`: whether it answered `wanted` (a prefix).
static int ask(const char *command, const char *wanted) {
	printf("discotest: asking b: %s\n", command);
	fflush(stdout);
	fprintf(control_out, "%s\n", command);
	fflush(control_out);
	char line[512];
	if(!fgets(line, sizeof(line), control_in)) {
		printf("discotest: FAILED: b did not answer `%s`\n", command);
		check_failures++;
		return 0;
	}
	line[strcspn(line, "\n")] = 0;
	printf("discotest: b: %s\n", line);
	if(strncmp(line, wanted, strlen(wanted)) != 0) {
		printf("discotest: FAILED: b answered `%s` with `%s`, not `%s`\n", command, line, wanted);
		check_failures++;
		return 0;
	}
	return 1;
}

static void names(void) {
	// Its own name, which discoverd answers for from its own addresses.
	const char *const own[] = {"64 bytes from a.local (192.168.50.1): icmp_seq=1", NULL};
	CHECK(wait_for("ping -4 -c 1 -W 5 a.local", own, 30));
	// b's, which b's discoverd answers on the cable, by IPv4 and IPv6.
	const char *const four[] = {"PING b.local (192.168.50.2) 56(84) bytes of data.",
			"64 bytes from b.local (192.168.50.2): icmp_seq=1", NULL};
	CHECK(wait_for("ping -4 -c 1 -W 5 b.local", four, 60));
	const char *const six[] = {"64 bytes from b.local (fd50::2): icmp_seq=1", NULL};
	CHECK(wait_for("ping -6 -c 1 -W 5 b.local", six, 60));
	const char *const asked[] = {"192.168.50.2", NULL};
	CHECK(wait_for("resolvectl query b.local", asked, 30));
	// And b finds a's.
	CHECK(ask("ping a.local", "ok 64 bytes from a.local (192.168.50.1)"));
	// A name no machine answers for.
	char said[2048];
	CHECK(output("ping -c 1 nosuch.local", said, sizeof(said)) == 2);
	printf("discotest: ping -c 1 nosuch.local said: %s", said);
	printf("discotest: a and b found each other's names\n");
}

static void services(void) {
	const char *const found[] = {"SERVICE", "_workstation._tcp", "b.local:9", "192.168.50.2",
			"DEVICE", ENDPOINT, "wsdp:Device pub:Computer", NULL};
	CHECK(wait_for("discover -w 3", found, 60));
	const char *const raw[] = {"service\tb\t_workstation._tcp\tb.local\t9\t", NULL};
	CHECK(wait_for("discover -r -w 2 _workstation._tcp", raw, 30));
	printf("discotest: discover listed b's workstation and b as a WS-Discovery device\n");
}

// Deletes the network capabilities but the one in the last slot, which
// it first derives from the first with `rights` for every port.
static void only(uint64_t rights) {
	uint32_t slot = RADIX_STARTUP_SLOT_NETWORK + RADIX_STARTUP_NETWORK_SLOTS - 1;
	radix_capability_delete(slot);
	uint64_t kept = RADIX_RIGHTS_COPY | RADIX_RIGHTS_TRANSFER;
	CHECK(radix_network_restrict(RADIX_STARTUP_SLOT_NETWORK, slot, 0, 65536, rights | kept) == 0);
	for(uint32_t i = 0; i < RADIX_STARTUP_NETWORK_SLOTS; i++)
		if(RADIX_STARTUP_SLOT_NETWORK + i != slot)
			radix_capability_delete(RADIX_STARTUP_SLOT_NETWORK + i);
}

// Pings b at 192.168.50.2 from a ping socket: the errno of sending, or 0
// once the reply came.
static int ping_b(void) {
	int fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_ICMP);
	if(fd < 0)
		return errno;
	struct sockaddr_in to = {.sin_family = AF_INET};
	inet_pton(AF_INET, "192.168.50.2", &to.sin_addr);
	unsigned char request[16] = {8, 0, 0, 0, 0, 0, 0, 1, 'l', 'o', 'c', 'a', 'l', '!', '!', '!'};
	if(sendto(fd, request, sizeof(request), 0, (struct sockaddr *)&to, sizeof(to)) < 0) {
		int error = errno;
		close(fd);
		return error;
	}
	struct pollfd p = {fd, POLLIN, 0};
	unsigned char reply[64];
	int got = poll(&p, 1, 5000) == 1 && recv(fd, reply, sizeof(reply), 0) > 0 && reply[0] == 0;
	close(fd);
	return got ? 0 : ETIMEDOUT;
}

static void rights(void) {
	pid_t pid = check_fork();
	if(pid == 0) {
		only(RADIX_RIGHTS_SEND);
		CHECK(ping_b() == EACCES);
		check_exit_child();
	}
	CHECK_CHILD(pid);
	pid = check_fork();
	if(pid == 0) {
		only(RADIX_RIGHTS_LOCAL);
		CHECK(ping_b() == 0);
		check_exit_child();
	}
	CHECK_CHILD(pid);
	printf("discotest: b was reached with the local right, not with the send right alone\n");
}

static int a(void) {
	printf("discotest: set_up\n");
	set_up("a", 1);
	int server = socket(AF_INET6, SOCK_STREAM, 0);
	int yes = 1;
	setsockopt(server, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));
	struct sockaddr_in6 at = {.sin6_family = AF_INET6, .sin6_port = htons(CONTROL_PORT)};
	CHECK(bind(server, (struct sockaddr *)&at, sizeof(at)) == 0);
	CHECK(listen(server, 1) == 0);
	printf("discotest: waiting for b on port %d\n", CONTROL_PORT);
	fflush(stdout);
	struct pollfd p = {server, POLLIN, 0};
	CHECK(poll(&p, 1, 300000) == 1);
	int fd = accept(server, NULL, NULL);
	CHECK(fd >= 0);
	close(server);
	if(fd >= 0) {
		control_in = fdopen(fd, "r");
		control_out = fdopen(dup(fd), "w");
		RUN(names);
		RUN(services);
		RUN(rights);
		CHECK(ask("done", "bye"));
		fclose(control_out);
		fclose(control_in);
	}
	return check_end();
}

// Does what a asks, from `from`, and answers it on `to`.
static void follow(FILE *from, FILE *to) {
	char line[512];
	while(fgets(line, sizeof(line), from)) {
		line[strcspn(line, "\n")] = 0;
		printf("discotest: a asks: %s\n", line);
		char reply[600];
		if(!strncmp(line, "ping ", 5) && !strchr(line + 5, '\'')) {
			char command[300], said[2048];
			snprintf(command, sizeof(command), "ping -4 -c 1 -W 5 '%s'", line + 5);
			int code = output(command, said, sizeof(said));
			const char *reply_line = strstr(said, "64 bytes from ");
			snprintf(reply, sizeof(reply), "%s %.*s", code == 0 ? "ok" : "failed",
					reply_line ? (int)strcspn(reply_line, "\n") : (int)strcspn(said, "\n"),
					reply_line ? reply_line : said);
		} else if(!strcmp(line, "done")) {
			fprintf(to, "bye\n");
			fflush(to);
			return;
		} else {
			snprintf(reply, sizeof(reply), "what is `%s`?", line);
		}
		printf("discotest: answering: %s\n", reply);
		fprintf(to, "%s\n", reply);
		fflush(to);
	}
	printf("discotest: FAILED: a went before it was done\n");
	check_failures++;
}

static int b(void) {
	printf("discotest: set_up\n");
	set_up("b", 2);
	pid_t probes = answer_probes();
	int fd = -1;
	for(int i = 0; i < 300 && fd < 0; i++) {
		fd = socket(AF_INET, SOCK_STREAM, 0);
		struct sockaddr_in to = {.sin_family = AF_INET, .sin_port = htons(CONTROL_PORT)};
		inet_pton(AF_INET, "192.168.50.1", &to.sin_addr);
		if(connect(fd, (struct sockaddr *)&to, sizeof(to)) < 0) {
			close(fd);
			fd = -1;
			check_sleep_ms(1000);
		}
	}
	if(fd < 0)
		printf("discotest: FAILED: cannot reach a on port %d\n", CONTROL_PORT);
	CHECK(fd >= 0);
	if(fd >= 0) {
		printf("discotest: connected to a\n");
		FILE *from = fdopen(fd, "r");
		FILE *to = fdopen(dup(fd), "w");
		follow(from, to);
		fclose(to);
		fclose(from);
	}
	kill(probes, SIGKILL);
	check_wait(probes);
	return check_end();
}

int main(int argc, char **argv) {
	check_start("discotest");
	if(argc == 2 && !strcmp(argv[1], "a"))
		return a();
	if(argc == 2 && !strcmp(argv[1], "b"))
		return b();
	printf("usage: discotest a | b\n");
	return 2;
}
