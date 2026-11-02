// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the spark-network boot test runs as its services (ADR 0184): it
// checks that Spark gave it the network its service file asks for, and
// nothing more. With `granted` that is a capability to connect out, to
// the local networks and beyond them (the local and send rights, ADR
// 0337), then one to listen on ports 8000 to 8009 and one on port 9000,
// both with the inbound right, which lets other machines reach them
// through the firewall (ADR 0334), and the C library's sockets go
// through them: it listens on ports 8005 and 9000 and connects to the
// first, but cannot bind port 8010. With `bundle`, what the bundle-run
// boot test's second bundle runs, it is the same but for the inbound
// right, which bundles never get, and the local right, which it did not
// ask for: it connects beyond the local networks alone, and does not see
// discoverd's folder. With `nearby`, the third bundle's, it holds the
// local right alone, which network.local gives, and sees discoverd's
// folder and asks it a question (ADR 0338). With `none` it holds no
// network capability, and cannot make a socket.

#include <arpa/inet.h>
#include <netinet/in.h>
#include <radix/radix.h>
#include <sys/socket.h>
#include <sys/un.h>

#include "check.h"

// What a network slot should hold: the rights and the ports, or nothing
// for an empty one (rights 0).
struct wanted {
	uint64_t rights;
	uint32_t first;
	uint32_t count;
};

// 127.0.0.1 and `port`.
static struct sockaddr_in loopback(unsigned short port) {
	struct sockaddr_in in;
	memset(&in, 0, sizeof(in));
	in.sin_family = AF_INET;
	in.sin_port = htons(port);
	in.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
	return in;
}

// Checks that the network slots hold what `wanted` says, in order, and
// that the rest are empty.
static void holds(const struct wanted *wanted, size_t count) {
	for(uint32_t i = 0; i < RADIX_STARTUP_NETWORK_SLOTS; i++) {
		uint32_t slot = RADIX_STARTUP_SLOT_NETWORK + i;
		struct radix_capability_info_results info;
		uint64_t status = radix_capability_info(slot, &info);
		if(i >= count) {
			if(status == 0)
				printf("netgrant: FAILED: slot %u holds a capability of kind %u\n", slot,
						(unsigned)info.kind);
			CHECK(status != 0);
			continue;
		}
		CHECK(status == 0);
		if(status)
			continue;
		CHECK(info.kind == RADIX_OBJECT_KIND_NETWORK);
		if(info.rights != wanted[i].rights)
			printf("netgrant: FAILED: slot %u has the rights %#llx, not %#llx\n", slot,
					(unsigned long long)info.rights, (unsigned long long)wanted[i].rights);
		CHECK(info.rights == wanted[i].rights);
		struct radix_network_ports_results ports;
		CHECK(radix_network_ports(slot, &ports) == 0);
		if(ports.first != wanted[i].first || ports.count != wanted[i].count)
			printf("netgrant: FAILED: slot %u covers %u ports from %u\n", slot,
					(unsigned)ports.count, (unsigned)ports.first);
		CHECK(ports.first == wanted[i].first && ports.count == wanted[i].count);
	}
}

// A TCP socket bound to 127.0.0.1 `port` and listening there.
static int listening(unsigned short port) {
	int fd = socket(AF_INET, SOCK_STREAM, 0);
	CHECK(fd >= 0);
	struct sockaddr_in at = loopback(port);
	CHECK(bind(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	CHECK(listen(fd, 1) == 0);
	return fd;
}

// discoverd's folder, which a bundle sees only with network.local.
#define DISCOVERD "/system/live/discoverd"

// Checks the capabilities of `granted` or `bundle`: the one to connect
// out with `out`, the listening ones with `inbound` among their rights;
// and that they work.
static void listens(uint64_t out, uint64_t inbound) {
	uint64_t kept = RADIX_RIGHTS_COPY | RADIX_RIGHTS_TRANSFER;
	const struct wanted wanted[] = {
			{out | kept, 0, 65536},
			{RADIX_RIGHTS_RECEIVE | inbound | kept, 8000, 10},
			{RADIX_RIGHTS_RECEIVE | inbound | kept, 9000, 1},
	};
	holds(wanted, sizeof(wanted) / sizeof(wanted[0]));
	// It listens on its ports, through the second capability and the
	// third, and connects out through the first.
	int server = listening(8005);
	int other = listening(9000);
	int client = socket(AF_INET, SOCK_STREAM, 0);
	CHECK(client >= 0);
	struct sockaddr_in to = loopback(8005);
	CHECK(connect(client, (struct sockaddr *)&to, sizeof(to)) == 0);
	int accepted = accept(server, NULL, NULL);
	CHECK(accepted >= 0);
	CHECK(write(client, "granted", 7) == 7);
	char buffer[8];
	CHECK(read(accepted, buffer, sizeof(buffer)) == 7 && memcmp(buffer, "granted", 7) == 0);
	close(accepted);
	close(client);
	close(other);
	close(server);
	// No capability covers port 8010.
	int fd = socket(AF_INET, SOCK_STREAM, 0);
	struct sockaddr_in at = loopback(8010);
	CHECK_ERROR(bind(fd, (struct sockaddr *)&at, sizeof(at)), EACCES);
	close(fd);
	printf("netgrant: Spark gave it the network to connect out and to listen on ports "
		   "8000-8009 and 9000\n");
}

static void granted(void) {
	listens(RADIX_RIGHTS_SEND | RADIX_RIGHTS_LOCAL, RADIX_RIGHTS_INBOUND);
}

static void bundle(void) {
	listens(RADIX_RIGHTS_SEND, 0);
	CHECK_ERROR(access(DISCOVERD, F_OK), ENOENT);
}

static void nearby(void) {
	uint64_t kept = RADIX_RIGHTS_COPY | RADIX_RIGHTS_TRANSFER;
	const struct wanted wanted[] = {{RADIX_RIGHTS_LOCAL | kept, 0, 65536}};
	holds(wanted, 1);
	// This machine, which the local right reaches as the send right does.
	int d = socket(AF_INET, SOCK_DGRAM, 0);
	struct sockaddr_in to = loopback(9);
	CHECK(sendto(d, "near", 4, 0, (struct sockaddr *)&to, sizeof(to)) == 4);
	close(d);
	// discoverd, which answers a question about this machine's own name.
	int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
	struct sockaddr_un at = {.sun_family = AF_UNIX, .sun_path = DISCOVERD "/socket"};
	CHECK(connect(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	const char question[] = "resolve dukovo.local\n";
	CHECK(write(fd, question, sizeof(question) - 1) == (ssize_t)sizeof(question) - 1);
	char answer[256];
	ssize_t got;
	while((got = read(fd, answer, sizeof(answer))) > 0) {
	}
	CHECK(got == 0);
	close(fd);
	printf("netgrant: Spark gave it the local network and discoverd\n");
}

static void none(void) {
	holds(NULL, 0);
	CHECK_ERROR(socket(AF_INET, SOCK_STREAM, 0), EACCES);
	CHECK_ERROR(socket(AF_INET, SOCK_DGRAM, 0), EACCES);
	CHECK_ERROR(access(DISCOVERD, F_OK), ENOENT);
	printf("netgrant: Spark gave it no network\n");
}

int main(int argc, char **argv) {
	check_start("netgrant");
	if(argc == 2 && strcmp(argv[1], "granted") == 0)
		RUN(granted);
	else if(argc == 2 && strcmp(argv[1], "bundle") == 0)
		RUN(bundle);
	else if(argc == 2 && strcmp(argv[1], "nearby") == 0)
		RUN(nearby);
	else if(argc == 2 && strcmp(argv[1], "none") == 0)
		RUN(none);
	else
		check_failed(__FILE__, __LINE__, "its argument is granted, bundle, nearby or none", EINVAL);
	int code = check_end();
	// The last line, which the boot test waits for, either way.
	printf("netgrant: done\n");
	return code;
}
