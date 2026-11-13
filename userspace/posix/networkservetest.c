// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The network-serve boot test's program (ADR 0486): two machines on a
// cable, a's card an e1000e at 10.9.0.1 and b's a virtio card, which a
// serves by DHCP and whose clock a sets over NTS. The structure, a
// control port that a drives b through, is that of logforwardtest.c.
//
// a gives each card on the cable's network an address from 10.9.0.100 to
// 10.9.0.150 for a minute, and b's, whose MAC address its settings
// reserve, 10.9.0.20 as `b`. The router, the DNS servers and the domain
// are set; the time server is a's while its timed answers plain NTP,
// which DHCP clients speak; the connection is metered. a's timed serves NTS and plain NTP with a certificate for
// a.test that an authority of its own signed, on a clock that nothing
// set (`serve-unsynced`). b's clock starts an hour ahead (the case's
// -rtc), and b's timed asks a.test, which b's hosts file gives, and
// trusts the authority.
//
// b connects to a's control port and does what a asks, a line each:
//
// - `run COMMAND`: runs it with sh(1), and says `ok CODE FIRST-LINE`;
// - `put PATH LENGTH`: writes the LENGTH bytes that follow to PATH, and
//   says `ok`;
// - `wait SECONDS NEEDLE|COMMAND`: runs the command until what it writes
//   holds the needle, and says `ok`, or `no` after that many seconds;
// - `time`: says `ok` and b's clock, in seconds since 1970;
// - `ntp ADDRESS`: asks the NTP server there once over UDP, and says `ok
//   stratum N`, or `no` when none answers;
// - `dhcp discover MAC`, `dhcp request MAC ADDRESS` and `dhcp release MAC
//   ADDRESS`: do what a DHCP client with that MAC address does, from
//   port 68 of net0, which is free while netd on b is stopped, and say
//   `ok` and what a's server answered, or `no`;
// - `done`: says `bye` and ends.
//
// a checks that b got 10.9.0.20 with what is set, and that its netd
// says the connection is metered; that b's timed took the time from a
// over NTS and stepped b's clock, which was an hour ahead, to within a
// second of a's; that b renewed its lease at the half of it, and a's
// list of the leases shows the new end; that a plain NTP query gets an
// answer from a's timed at stratum 10, and none once it serves NTS only;
// and that a second client, a MAC address of its own in chaddr and in
// option 61, is offered the lowest address of the pool, gets it, and has
// it back at its release, as a's netd logs.

#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/stat.h>

#include "check.h"

#define CONTROL_PORT 7000
#define A_ADDRESS "10.9.0.1"
#define B_ADDRESS "10.9.0.20"
#define B_MAC "52:54:00:12:34:a1"
#define OTHER_MAC "52:54:00:99:00:01"
#define POOL_FIRST "10.9.0.100"
#define NTS_PORT "4460"
// What netd writes of the leases it gave and of the connection, and the
// lease file of b's card, which is named by its MAC address.
#define LEASES "/system/live/netd/leases"
#define METERED "/system/live/netd/metered"
#define B_LEASE "/system/state/netd/52-54-00-12-34-a1.lease"
#define SSL "/system/settings/ssl"
#define TIMED_SETTINGS "/system/settings/timed"

// Runs `command` with sh(1), saying so: its exit code, or -1.
static int run(const char *command) {
	printf("networkservetest: $ %s\n", command);
	fflush(stdout);
	int status = system(command);
	return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

// Runs `command` with sh(1) and keeps what it writes, its standard error
// too, in `buffer`: its exit code, or -1.
static int output(const char *command, char *buffer, size_t size) {
	char line[1536];
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
	do {
		if(output(command, buffer, sizeof(buffer)) == 0 && strstr(buffer, needle))
			return 1;
		check_sleep_ms(500);
	} while(check_now_ns() < deadline);
	printf("networkservetest: FAILED: `%s` never said \"%s\"; it said: %s\n", command, needle,
			buffer);
	check_failures++;
	return 0;
}

// Writes `text` to `path`.
static void write_file(const char *path, const char *text) {
	FILE *file = fopen(path, "w");
	CHECK(file && fputs(text, file) >= 0 && fclose(file) == 0);
}

// What the file at `path` holds, in memory that is the caller's, or NULL.
static char *read_file(const char *path) {
	FILE *file = fopen(path, "r");
	if(!file)
		return NULL;
	char *text = malloc(65536);
	size_t length = text ? fread(text, 1, 65535, file) : 0;
	fclose(file);
	if(text)
		text[length] = 0;
	return text;
}

// A socket of `type` bound to every IPv4 address at `port`; a TCP one
// listening.
static int bound(int type, unsigned short port) {
	int fd = socket(AF_INET, type, 0);
	CHECK(fd >= 0);
	int yes = 1;
	setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));
	struct sockaddr_in at;
	memset(&at, 0, sizeof(at));
	at.sin_family = AF_INET;
	at.sin_port = htons(port);
	at.sin_addr.s_addr = htonl(INADDR_ANY);
	CHECK(bind(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	if(type == SOCK_STREAM)
		CHECK(listen(fd, 8) == 0);
	return fd;
}

// --- machine a: what it serves ------------------------------------------

// A certificate authority and a certificate for a.test that it signed,
// with its key and chain where timed(5) reads them; the authority's
// certificate is /tmp/ca.pem, for b.
static void make_certificates(void) {
	const char *key = "openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:prime256v1 -out ";
	char command[512];
	write_file("/tmp/a.ext",
			"subjectAltName=DNS:a.test\nbasicConstraints=critical,CA:FALSE\n"
			"keyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n");
	CHECK(run("mkdir -p " SSL) == 0);
	snprintf(command, sizeof(command), "%s/tmp/ca.key", key);
	CHECK(run(command) == 0);
	CHECK(run("openssl req -new -x509 -key /tmp/ca.key -subj /CN=serve-test-ca -days 3650 "
			  "-out /tmp/ca.pem") == 0);
	snprintf(command, sizeof(command), "%s" SSL "/a.key", key);
	CHECK(run(command) == 0);
	CHECK(run("openssl req -new -key " SSL "/a.key -subj /CN=a.test -out /tmp/a.csr") == 0);
	CHECK(run("openssl x509 -req -in /tmp/a.csr -CA /tmp/ca.pem -CAkey /tmp/ca.key -set_serial 2 "
			  "-days 3650 -extfile /tmp/a.ext -out " SSL "/a.pem") == 0);
	CHECK(run("openssl verify -CAfile /tmp/ca.pem " SSL "/a.pem") == 0);
}

// Writes timed's settings so that it serves the time as `mode` says, `nts`
// or `nts+ntp`, from a clock of its own, with no server to ask.
static void time_settings(const char *mode) {
	char text[512];
	snprintf(text, sizeof(text),
			"servers =\nfallback =\nserve = %s\nserve-certificate = " SSL "/a.pem\n"
			"serve-key = " SSL "/a.key\nserve-unsynced = yes\n",
			mode);
	write_file(TIMED_SETTINGS, text);
}

// Starts timed again, which reads its settings as it starts, and waits for
// it to listen for key establishment.
static void restart_time(void) {
	CHECK(run("service timed restart") == 0);
	CHECK(wait_for("nc -z -w 1 " A_ADDRESS " " NTS_PORT "; echo listening=$?", "listening=0", 30));
}

// The card has its address and gives the pool's: the settings of the
// card are network(5)'s.
static void serve_addresses(void) {
	write_file("/system/settings/network",
			"[net0]\n"
			"dhcp = no\n"
			"address = " A_ADDRESS "/24\n"
			"dhcp-server = 10.9.0.100-10.9.0.150\n"
			"dhcp-server-lease = 1m\n"
			"dhcp-server-router = 10.9.0.2\n"
			"dhcp-server-dns = 10.9.0.53 10.9.0.54\n"
			"dhcp-server-domain = serve.test\n"
			"dhcp-server-metered = yes\n"
			"reserve = " B_MAC " " B_ADDRESS " b\n");
	CHECK(run("service netd reload") == 0);
	CHECK(wait_for("ip -4 address show dev net0", A_ADDRESS "/24", 60));
}

// --- machine a: asking b ------------------------------------------------

// Machine a's connection to machine b, a stream each way.
static FILE *control_in, *control_out;
// What b answered last.
static char answer[2048];

// Reads b's answer to `command`: whether it starts with `wanted`.
static int answered(const char *command, const char *wanted) {
	if(!fgets(answer, sizeof(answer), control_in)) {
		printf("networkservetest: FAILED: b did not answer `%s`\n", command);
		check_failures++;
		answer[0] = 0;
		return 0;
	}
	answer[strcspn(answer, "\n")] = 0;
	printf("networkservetest: b: %s\n", answer);
	if(strncmp(answer, wanted, strlen(wanted)) != 0) {
		printf("networkservetest: FAILED: b answered `%s` with `%s`, not `%s`\n", command, answer,
				wanted);
		check_failures++;
		return 0;
	}
	return 1;
}

// Asks b to do `command`: whether it answered `wanted` (a prefix).
static int ask(const char *command, const char *wanted) {
	printf("networkservetest: asking b: %s\n", command);
	fflush(stdout);
	fprintf(control_out, "%s\n", command);
	fflush(control_out);
	return answered(command, wanted);
}

// Asks b to do `command` and checks that its answer holds `needle`.
static int ask_says(const char *command, const char *wanted, const char *needle) {
	if(!ask(command, wanted))
		return 0;
	if(!strstr(answer, needle)) {
		printf("networkservetest: FAILED: b answered `%s` with `%s`, which has no \"%s\"\n", command,
				answer, needle);
		check_failures++;
		return 0;
	}
	return 1;
}

// Has b write `text` to `path`.
static int put(const char *path, const char *text) {
	char command[320];
	snprintf(command, sizeof(command), "put %s %zu", path, strlen(text));
	printf("networkservetest: asking b: %s\n", command);
	fflush(stdout);
	fprintf(control_out, "%s\n", command);
	fwrite(text, 1, strlen(text), control_out);
	fflush(control_out);
	return answered(command, "ok");
}

// Asks b to wait until `command` says `needle`, for `seconds`.
static int ask_wait(int seconds, const char *needle, const char *command) {
	char line[1024];
	snprintf(line, sizeof(line), "wait %d %s|%s", seconds, needle, command);
	return ask(line, "ok");
}

// Asks b to do `command` each second, for at most `seconds`, until its
// answer starts with `wanted`, as netd takes its settings again some time
// after `service netd reload` returns; only the last answer can fail.
static int ask_until(int seconds, const char *command, const char *wanted) {
	for(int tries = 1; tries < seconds; tries++) {
		fprintf(control_out, "%s\n", command);
		fflush(control_out);
		if(!fgets(answer, sizeof(answer), control_in))
			break;
		answer[strcspn(answer, "\n")] = 0;
		if(strncmp(answer, wanted, strlen(wanted)) == 0) {
			printf("networkservetest: b: %s\n", answer);
			return 1;
		}
		check_sleep_ms(1000);
	}
	return ask(command, wanted);
}

// b's clock, in seconds since 1970; 0 if it did not say.
static long long b_time(void) {
	return ask("time", "ok ") ? atoll(answer + 3) : 0;
}

// Where in the list of leases (`netd leases` reads it) the client of the
// MAC address `mac` is: when its lease ends, in seconds since 1970, 0 if
// it only has an address reserved, or -1 if it is not there.
static long long lease_end(const char *mac) {
	char *text = read_file(LEASES);
	long long ends = -1;
	for(const char *line = text; line && *line;) {
		const char *end = strchr(line, '\n');
		size_t length = end ? (size_t)(end - line) : strlen(line);
		char row[256], card[32], address[32], client[32], name[64], when[32];
		snprintf(row, sizeof(row), "%.*s", (int)length, line);
		if(sscanf(row, "%31s %31s %31s %63s %31s", card, address, client, name, when) == 5
				&& !strcmp(client, mac))
			ends = strcmp(when, "reserved") ? atoll(when) : 0;
		line = end ? end + 1 : NULL;
	}
	free(text);
	return ends;
}

// Waits for the list to show a lease of the client of `mac`, not only an
// address reserved for it, for `seconds`: when it ends.
static long long wait_for_lease(const char *mac, int seconds) {
	long long deadline = check_now_ns() + seconds * 1000000000LL;
	long long ends = lease_end(mac);
	while(ends <= 0 && check_now_ns() < deadline) {
		check_sleep_ms(500);
		ends = lease_end(mac);
	}
	if(ends <= 0) {
		printf("networkservetest: FAILED: the list of leases never showed one of %s\n", mac);
		check_failures++;
	}
	return ends;
}

// --- machine a's parts --------------------------------------------------

// b has its address, with the router, the DNS servers and the domain
// that are set, and a lists the lease by name.
static void lease(void) {
	CHECK(wait_for_lease(B_MAC, 60) > 0);
	CHECK(wait_for("netd leases", B_ADDRESS, 10));
	CHECK(wait_for("netd leases", " b ", 10));
	CHECK(ask_wait(30, "address = " B_ADDRESS "/24", "cat " B_LEASE));
	CHECK(ask_wait(10, "router = 10.9.0.2", "cat " B_LEASE));
	CHECK(ask_wait(10, "dns = 10.9.0.53", "cat " B_LEASE));
	CHECK(ask_wait(10, "dns = 10.9.0.54", "cat " B_LEASE));
	CHECK(ask_wait(10, "domain = serve.test", "cat " B_LEASE));
	CHECK(ask_wait(10, "server = " A_ADDRESS, "cat " B_LEASE));
	printf("networkservetest: b got " B_ADDRESS " with the router, DNS servers and domain that are "
		   "set, and a lists it by name\n");
}

// The option 43 of the lease says the connection is metered, and netd
// on b says so to updated.
static void metered(void) {
	CHECK(ask_wait(10, "metered = yes", "cat " B_LEASE));
	CHECK(ask("run head -n 1 " METERED, "ok 0 yes"));
	printf("networkservetest: b's netd says the connection is metered\n");
}

// b's clock started an hour ahead of a's; its timed asks a over NTS and
// steps it.
static void time_over_nts(void) {
	long long ahead = b_time() - (long long)time(NULL);
	printf("networkservetest: b's clock is %lld s ahead of a's\n", ahead);
	CHECK(ahead > 3500 && ahead < 3700);
	CHECK(ask("run mkdir -p " SSL, "ok 0"));
	char *authority = read_file("/tmp/ca.pem");
	CHECK(authority != NULL);
	if(authority)
		CHECK(put(SSL "/test-ca.pem", authority));
	free(authority);
	CHECK(put(TIMED_SETTINGS,
			"servers = a.test:" NTS_PORT "\nfallback =\ncertificates = " SSL "/test-ca.pem\n"));
	CHECK(ask("run echo " A_ADDRESS " a.test >> /system/settings/hosts", "ok 0"));
	CHECK(ask("run service timed restart", "ok 0"));
	CHECK(ask_wait(120, "from a.test:" NTS_PORT " over NTS", "timedatectl timesync-status"));
	long long difference = b_time() - (long long)time(NULL);
	printf("networkservetest: b's clock is %lld s from a's\n", difference);
	CHECK(difference >= -1 && difference <= 1);
	printf("networkservetest: b started an hour ahead, and timed took the time from a over NTS and "
		   "stepped the clock to within a second of a's\n");
}

// b renewed its lease at the half of it, a minute's 30 seconds, and the
// list shows the end moved on.
static void renewal(long long first_end) {
	long long deadline = check_now_ns() + 90 * 1000000000LL;
	long long end = lease_end(B_MAC);
	while(end <= first_end && check_now_ns() < deadline) {
		check_sleep_ms(1000);
		end = lease_end(B_MAC);
	}
	printf("networkservetest: b's lease ended at %lld, now at %lld\n", first_end, end);
	CHECK(first_end > 0 && end > first_end);
	printf("networkservetest: b renewed its lease at the half of it, and the end moved on\n");
}

// A plain NTP query to a gets an answer from a server at stratum 10,
// which is what a server with no upstream of its own gives that
// `serve-unsynced` lets serve, and, with a serving NTS alone, none.
static void plain_ntp(void) {
	CHECK(ask_says("ntp " A_ADDRESS, "ok stratum", "stratum 10"));
	time_settings("nts");
	restart_time();
	CHECK(ask("ntp " A_ADDRESS, "no"));
	printf("networkservetest: a plain NTP query got an answer from a stratum 10 server, and none "
		   "once it served NTS alone\n");
}

// What the offer made to a client, one line, says: the address and what
// the settings gave with it, with a as the time server while its timed
// answers plain NTP, which DHCP clients speak, and none while it serves
// NTS alone.
#define OFFER_OTHER \
	"ok offer " POOL_FIRST " server " A_ADDRESS \
	" lease 60 router 10.9.0.2 dns 10.9.0.53,10.9.0.54 time " A_ADDRESS \
	" domain serve.test metered"
#define OFFER_OTHER_NO_TIME \
	"ok offer " POOL_FIRST " server " A_ADDRESS \
	" lease 60 router 10.9.0.2 dns 10.9.0.53,10.9.0.54 domain serve.test metered"

// A client of its own MAC address, with netd on b stopped so that its
// port 68 is free, is offered the pool's lowest address with what is
// set, gets it, and gives it back; b's own MAC address is offered the
// address reserved for it. The offer names a as the time server only
// while a's timed answers plain NTP: not while it serves NTS alone, as
// plain_ntp left it. a's netd reloads its settings, which leave a's
// address as it was, with the connection to b open: it must stay.
static void other_client(void) {
	CHECK(ask("run service netd stop", "ok 0"));
	CHECK(run("service netd reload") == 0);
	CHECK(ask_until(10, "dhcp discover " OTHER_MAC, OFFER_OTHER_NO_TIME));
	time_settings("nts+ntp");
	restart_time();
	CHECK(run("service netd reload") == 0);
	CHECK(ask_until(10, "dhcp discover " OTHER_MAC, OFFER_OTHER));
	CHECK(ask("dhcp discover " B_MAC, "ok offer " B_ADDRESS " server " A_ADDRESS));
	CHECK(ask("dhcp request " OTHER_MAC " " POOL_FIRST, "ok ack " POOL_FIRST));
	CHECK(wait_for_lease(OTHER_MAC, 10) > 0);
	CHECK(wait_for("cat " LEASES, POOL_FIRST " " OTHER_MAC, 5));
	CHECK(ask("dhcp release " OTHER_MAC " " POOL_FIRST, "ok"));
	long long deadline = check_now_ns() + 10 * 1000000000LL;
	while(lease_end(OTHER_MAC) >= 0 && check_now_ns() < deadline)
		check_sleep_ms(500);
	CHECK(lease_end(OTHER_MAC) < 0);
	CHECK(ask("run service netd start", "ok 0"));
	CHECK(ask_wait(60, B_ADDRESS "/24", "ip -4 address show dev net0"));
	printf("networkservetest: a second client was offered the lowest address of the pool with what "
		   "is set, with a time server only while a answered plain NTP, got it, and gave it back\n");
}

static int a(void) {
	RUN(make_certificates);
	// The settings of the time come first, as netd gives the time server
	// of timed's the clients that ask, which it finds in them.
	time_settings("nts+ntp");
	serve_addresses();
	restart_time();
	int server = bound(SOCK_STREAM, CONTROL_PORT);
	printf("networkservetest: waiting for b on port %d\n", CONTROL_PORT);
	fflush(stdout);
	struct pollfd p = {server, POLLIN, 0};
	CHECK(poll(&p, 1, 300000) == 1);
	int fd = accept(server, NULL, NULL);
	CHECK(fd >= 0);
	close(server);
	if(fd >= 0) {
		control_in = fdopen(fd, "r");
		control_out = fdopen(dup(fd), "w");
		RUN(lease);
		RUN(metered);
		long long first_end = lease_end(B_MAC);
		RUN(time_over_nts);
		renewal(first_end);
		RUN(plain_ntp);
		RUN(other_client);
		CHECK(ask("done", "bye"));
		fclose(control_out);
		fclose(control_in);
	}
	return check_end();
}

// --- machine b ----------------------------------------------------------

// b's NTP query: asks the server at `address` once, for up to three
// seconds, with a request of an origin time that only its answer can
// repeat: whether one answered, and its stratum.
static int ntp_query(const char *address, int *stratum) {
	int fd = socket(AF_INET, SOCK_DGRAM, 0);
	CHECK(fd >= 0);
	unsigned char request[48] = {0x23};
	long long nonce = check_now_ns();
	memcpy(request + 40, &nonce, sizeof(nonce));
	struct sockaddr_in to;
	memset(&to, 0, sizeof(to));
	to.sin_family = AF_INET;
	to.sin_port = htons(123);
	inet_pton(AF_INET, address, &to.sin_addr);
	int answered = 0;
	if(sendto(fd, request, sizeof(request), 0, (struct sockaddr *)&to, sizeof(to)) == (ssize_t)sizeof(request)) {
		struct pollfd p = {fd, POLLIN, 0};
		unsigned char reply[512];
		ssize_t got;
		if(poll(&p, 1, 3000) == 1 && (got = recv(fd, reply, sizeof(reply), 0)) >= 48
				&& (reply[0] & 7) == 4 && !memcmp(reply + 24, request + 40, 8)) {
			*stratum = reply[1];
			answered = 1;
		}
	}
	close(fd);
	return answered;
}

// What a DHCP server's message says, of what this test looks for.
struct offer {
	int type;
	unsigned char address[4], server[4], router[4], ntp[4];
	unsigned lease;
	int has_server, has_router, has_ntp;
	unsigned char dns[12];
	int dns_count;
	char domain[64];
	int metered;
};

#define DHCP_FIXED 240
#define DHCP_DISCOVER 1
#define DHCP_OFFER 2
#define DHCP_REQUEST 3
#define DHCP_ACK 5
#define DHCP_RELEASE 7

// The four bytes at `at` as a number.
static unsigned be32(const unsigned char *at) {
	return (unsigned)at[0] << 24 | (unsigned)at[1] << 16 | (unsigned)at[2] << 8 | at[3];
}

// Whether the `size` bytes at `value` hold the text `text`.
static int holds(const unsigned char *value, size_t size, const char *text) {
	size_t length = strlen(text);
	for(size_t at = 0; at + length <= size; at++)
		if(!memcmp(value + at, text, length))
			return 1;
	return 0;
}

// Takes the option `code` of `size` bytes at `value` into `offer`, if it is
// one this test looks for.
static void dhcp_option(struct offer *offer, unsigned code, const unsigned char *value,
		size_t size) {
	if(code == 53 && size == 1)
		offer->type = value[0];
	else if(code == 54 && size == 4) {
		memcpy(offer->server, value, 4);
		offer->has_server = 1;
	} else if(code == 51 && size == 4)
		offer->lease = be32(value);
	else if(code == 3 && size >= 4) {
		memcpy(offer->router, value, 4);
		offer->has_router = 1;
	} else if(code == 42 && size >= 4) {
		memcpy(offer->ntp, value, 4);
		offer->has_ntp = 1;
	} else if(code == 15)
		snprintf(offer->domain, sizeof(offer->domain), "%.*s", (int)size, (const char *)value);
	else if(code == 43)
		offer->metered = holds(value, size, "ANDROID_METERED");
	else if(code == 6 && size >= 4) {
		offer->dns_count = (int)(size / 4 < 3 ? size / 4 : 3);
		memcpy(offer->dns, value, (size_t)offer->dns_count * 4);
	}
}

// Reads the BOOTP message `message` of `length` bytes, if it is a reply
// to the request `xid` of the client `mac`, into `offer`: its type, or 0.
static int dhcp_read(const unsigned char *message, size_t length, unsigned xid,
		const unsigned char mac[6], struct offer *offer) {
	if(length < DHCP_FIXED + 4 || message[0] != 2 || memcmp(message + 28, mac, 6) != 0
			|| be32(message + 4) != xid || memcmp(message + 236, "\x63\x82\x53\x63", 4) != 0)
		return 0;
	memset(offer, 0, sizeof(*offer));
	memcpy(offer->address, message + 16, 4);
	for(size_t at = DHCP_FIXED; at < length && message[at] != 255;) {
		if(message[at] == 0) {
			at++;
			continue;
		}
		if(at + 1 >= length || at + 2 + message[at + 1] > length)
			break;
		dhcp_option(offer, message[at], message + at + 2, message[at + 1]);
		at += 2 + message[at + 1];
	}
	return offer->type;
}

// What `offer` says as one line, its parts in this order: the address
// given, the server, the lease time in seconds, the router, the DNS
// servers, the time server, the domain and whether it says metered.
static void dhcp_text(const struct offer *offer, char *text, size_t size) {
	int at = snprintf(text, size, "%u.%u.%u.%u", offer->address[0], offer->address[1],
			offer->address[2], offer->address[3]);
	if(offer->has_server)
		at += snprintf(text + at, size - (size_t)at, " server %u.%u.%u.%u", offer->server[0],
				offer->server[1], offer->server[2], offer->server[3]);
	at += snprintf(text + at, size - (size_t)at, " lease %u", offer->lease);
	if(offer->has_router)
		at += snprintf(text + at, size - (size_t)at, " router %u.%u.%u.%u", offer->router[0],
				offer->router[1], offer->router[2], offer->router[3]);
	for(int i = 0; i < offer->dns_count; i++)
		at += snprintf(text + at, size - (size_t)at, "%s%u.%u.%u.%u", i ? "," : " dns ",
				offer->dns[4 * i], offer->dns[4 * i + 1], offer->dns[4 * i + 2],
				offer->dns[4 * i + 3]);
	if(offer->has_ntp)
		at += snprintf(text + at, size - (size_t)at, " time %u.%u.%u.%u", offer->ntp[0],
				offer->ntp[1], offer->ntp[2], offer->ntp[3]);
	if(offer->domain[0])
		at += snprintf(text + at, size - (size_t)at, " domain %s", offer->domain);
	if(offer->metered)
		snprintf(text + at, size - (size_t)at, " metered");
}

// Builds a client's message of `type` for the MAC address `mac` into
// `message`: the address it wants or has, if there is one, its server's
// identifier unless it is a DISCOVER, and the option 61 that names it.
// How many bytes.
static size_t dhcp_message(unsigned char *message, int type, const unsigned char mac[6],
		const unsigned char *address, unsigned xid) {
	static const unsigned char server[4] = {10, 9, 0, 1};
	memset(message, 0, 400);
	message[0] = 1;
	message[1] = 1;
	message[2] = 6;
	unsigned wire = htonl(xid);
	memcpy(message + 4, &wire, 4);
	// Broadcast flag: the answer comes to everyone's port 68.
	message[10] = 0x80;
	if(type == DHCP_RELEASE && address)
		memcpy(message + 12, address, 4);
	memcpy(message + 28, mac, 6);
	memcpy(message + 236, "\x63\x82\x53\x63", 4);
	unsigned char *at = message + DHCP_FIXED;
	*at++ = 53;
	*at++ = 1;
	*at++ = (unsigned char)type;
	*at++ = 61;
	*at++ = 7;
	*at++ = 1;
	memcpy(at, mac, 6);
	at += 6;
	if(type == DHCP_REQUEST && address) {
		*at++ = 50;
		*at++ = 4;
		memcpy(at, address, 4);
		at += 4;
	}
	if(type != DHCP_DISCOVER) {
		*at++ = 54;
		*at++ = 4;
		memcpy(at, server, 4);
		at += 4;
	}
	static const unsigned char wanted[] = {55, 9, 1, 3, 6, 15, 42, 43, 51, 54, 58};
	memcpy(at, wanted, sizeof(wanted));
	at += sizeof(wanted);
	*at++ = 255;
	return (size_t)(at - message);
}

// A socket on port 68 of net0, which may broadcast: the port is netd's
// while it runs.
static int dhcp_socket(void) {
	int fd = socket(AF_INET, SOCK_DGRAM, 0);
	if(fd < 0)
		return -1;
	int yes = 1;
	struct sockaddr_in at;
	memset(&at, 0, sizeof(at));
	at.sin_family = AF_INET;
	at.sin_port = htons(68);
	if(setsockopt(fd, SOL_SOCKET, SO_BROADCAST, &yes, sizeof(yes)) != 0
			|| setsockopt(fd, SOL_SOCKET, SO_BINDTODEVICE, "net0", 4) != 0
			|| bind(fd, (struct sockaddr *)&at, sizeof(at)) != 0) {
		close(fd);
		return -1;
	}
	return fd;
}

// Does what a DHCP client of the MAC address `mac_text` does for `verb`,
// `discover`, `request` or `release`, with `address_text`, and writes the
// line to answer a with into `reply`.
static void dhcp_do(const char *verb, const char *mac_text, const char *address_text, char *reply,
		size_t size) {
	unsigned char mac[6], address[4] = {0}, message[400];
	unsigned m[6], ip[4] = {0};
	if(sscanf(mac_text, "%x:%x:%x:%x:%x:%x", &m[0], &m[1], &m[2], &m[3], &m[4], &m[5]) != 6
			|| (address_text && sscanf(address_text, "%u.%u.%u.%u", &ip[0], &ip[1], &ip[2], &ip[3]) != 4)) {
		snprintf(reply, size, "what is that MAC address or address?");
		return;
	}
	for(int i = 0; i < 6; i++)
		mac[i] = (unsigned char)m[i];
	for(int i = 0; address_text && i < 4; i++)
		address[i] = (unsigned char)ip[i];
	int type = !strcmp(verb, "discover") ? DHCP_DISCOVER
			: !strcmp(verb, "request")   ? DHCP_REQUEST
			: !strcmp(verb, "release")   ? DHCP_RELEASE
										 : 0;
	int fd = type ? dhcp_socket() : -1;
	if(fd < 0) {
		snprintf(reply, size, "no: cannot be a client on port 68 (%s)", type ? strerror(errno) : verb);
		return;
	}
	unsigned xid = (unsigned)check_now_ns();
	size_t length = dhcp_message(message, type, mac, address_text ? address : NULL, xid);
	struct sockaddr_in to;
	memset(&to, 0, sizeof(to));
	to.sin_family = AF_INET;
	to.sin_port = htons(67);
	to.sin_addr.s_addr = htonl(type == DHCP_RELEASE ? 0x0a090001 : INADDR_BROADCAST);
	int want = type == DHCP_DISCOVER ? DHCP_OFFER : DHCP_ACK;
	struct offer offer;
	memset(&offer, 0, sizeof(offer));
	int done = 0;
	for(int attempt = 0; attempt < 3 && !done; attempt++) {
		if(sendto(fd, message, length, 0, (struct sockaddr *)&to, sizeof(to)) != (ssize_t)length)
			break;
		if(type == DHCP_RELEASE) {
			// Nothing answers a RELEASE.
			snprintf(reply, size, "ok sent");
			close(fd);
			return;
		}
		long long until = check_now_ns() + 2000000000LL;
		while(!done && check_now_ns() < until) {
			struct pollfd p = {fd, POLLIN, 0};
			unsigned char got[1500];
			ssize_t count;
			if(poll(&p, 1, 200) == 1 && (count = recv(fd, got, sizeof(got), 0)) > 0)
				done = dhcp_read(got, (size_t)count, xid, mac, &offer) == want;
		}
	}
	close(fd);
	if(done) {
		char text[400];
		dhcp_text(&offer, text, sizeof(text));
		snprintf(reply, size, "ok %s %s", want == DHCP_OFFER ? "offer" : "ack", text);
	} else
		snprintf(reply, size, "no: %s got no %s", verb, want == DHCP_OFFER ? "offer" : "ack");
}

// Does what a asks, from `from`, and answers it on `to`.
static void follow(FILE *from, FILE *to) {
	char line[1024];
	while(fgets(line, sizeof(line), from)) {
		line[strcspn(line, "\n")] = 0;
		printf("networkservetest: a asks: %s\n", line);
		char reply[1200];
		if(!strncmp(line, "run ", 4)) {
			static char said[16384];
			int code = output(line + 4, said, sizeof(said));
			printf("%s", said);
			said[strcspn(said, "\n")] = 0;
			snprintf(reply, sizeof(reply), "ok %d %.1000s", code, said);
		} else if(!strncmp(line, "put ", 4)) {
			char path[256];
			size_t length;
			if(sscanf(line + 4, "%255s %zu", path, &length) != 2 || length > 65536) {
				snprintf(reply, sizeof(reply), "what is `%.200s`?", line);
			} else {
				char *content = malloc(length + 1);
				size_t got = content ? fread(content, 1, length, from) : 0;
				FILE *file = got == length ? fopen(path, "w") : NULL;
				int wrote = file && fwrite(content, 1, length, file) == length;
				if(file && fclose(file) != 0)
					wrote = 0;
				free(content);
				snprintf(reply, sizeof(reply), "%s", wrote ? "ok" : "cannot write the file");
			}
		} else if(!strncmp(line, "wait ", 5)) {
			int seconds = atoi(line + 5);
			char *rest = strchr(line + 5, ' ');
			char *bar = rest ? strchr(rest + 1, '|') : NULL;
			if(!bar) {
				snprintf(reply, sizeof(reply), "what is `%.200s`?", line);
			} else {
				*bar = 0;
				int found = wait_for(bar + 1, rest + 1, seconds);
				snprintf(reply, sizeof(reply), "%s", found ? "ok" : "no");
			}
		} else if(!strcmp(line, "time")) {
			snprintf(reply, sizeof(reply), "ok %lld", (long long)time(NULL));
		} else if(!strncmp(line, "ntp ", 4)) {
			int stratum = 0;
			if(ntp_query(line + 4, &stratum))
				snprintf(reply, sizeof(reply), "ok stratum %d", stratum);
			else
				snprintf(reply, sizeof(reply), "no");
		} else if(!strncmp(line, "dhcp ", 5)) {
			char verb[16], mac[32], address[32];
			int words = sscanf(line + 5, "%15s %31s %31s", verb, mac, address);
			if(words < 2)
				snprintf(reply, sizeof(reply), "what is `%.200s`?", line);
			else
				dhcp_do(verb, mac, words == 3 ? address : NULL, reply, sizeof(reply));
		} else if(!strcmp(line, "done")) {
			fprintf(to, "bye\n");
			fflush(to);
			return;
		} else {
			snprintf(reply, sizeof(reply), "what is `%.200s`?", line);
		}
		printf("networkservetest: answering: %s\n", reply);
		fprintf(to, "%s\n", reply);
		fflush(to);
	}
	printf("networkservetest: FAILED: a went before it was done\n");
	check_failures++;
}

// Connects to a's control port once b has its address, waiting for a to
// listen: the socket, or -1. netd on b asks a for the address at boot.
static int reach_a(void) {
	for(int i = 0; i < 300; i++) {
		int fd = socket(AF_INET, SOCK_STREAM, 0);
		struct sockaddr_in to;
		memset(&to, 0, sizeof(to));
		to.sin_family = AF_INET;
		to.sin_port = htons(CONTROL_PORT);
		inet_pton(AF_INET, A_ADDRESS, &to.sin_addr);
		if(fd >= 0 && connect(fd, (struct sockaddr *)&to, sizeof(to)) == 0)
			return fd;
		if(fd >= 0)
			close(fd);
		check_sleep_ms(1000);
	}
	return -1;
}

static int b(void) {
	int fd = reach_a();
	if(fd < 0)
		printf("networkservetest: FAILED: cannot reach a on port %d\n", CONTROL_PORT);
	CHECK(fd >= 0);
	if(fd >= 0) {
		printf("networkservetest: connected to a\n");
		FILE *from = fdopen(fd, "r");
		FILE *to = fdopen(dup(fd), "w");
		follow(from, to);
		fclose(to);
		fclose(from);
	}
	return check_end();
}

int main(int argc, char **argv) {
	check_start("networkservetest");
	if(argc == 2 && !strcmp(argv[1], "a"))
		return a();
	if(argc == 2 && !strcmp(argv[1], "b"))
		return b();
	printf("usage: networkservetest a | b\n");
	return 2;
}
