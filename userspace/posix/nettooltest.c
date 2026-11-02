// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The network-tools boot test's program (ADRs 0341 and 0342): two
// machines on a cable, 192.168.50.1 and .2 and fd50::1 and ::2, a's card
// an e1000e and b's a virtio card. b serves what a's tools ask: a name
// server for tools.test on port 8053, over UDP and TCP and over UDP from
// IPv6 on 8054, and WHOIS servers on port 8043, which answer as IANA's,
// a registry's and a registrar's do in turn. b connects to a's control
// port and does what a asks, a line each:
//
// - `run COMMAND`: runs it with sh(1), and says `ok CODE FIRST-LINE`;
// - `start COMMAND`: starts it with sh(1) in the background, waits a
//   second for it to listen, and says `started`;
// - `done`: says `bye` and ends.
//
// a checks nic(8) on its e1000e, whose link QEMU gives as 1000 Mb/s full
// duplex; nc(1) both ways, over TCP and UDP and by IPv4 and IPv6, and
// its port scan; iperf3(1) against b's server, over TCP, UDP, IPv6 and
// the other way; dig(1) and host(1) against b's name server, and host
// through resolved(8), which a's settings point at b's; whois(1)
// following the referrals from "IANA's" server through the names b's
// name server gives, and asking ARIN's for networks; and openssl(1)'s
// keys, certificates and digests, with b's s_client reaching a's
// s_server over TLS 1.3. b checks nic on its virtio card, which says no
// speed.

#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/socket.h>

#include "check.h"

#define CONTROL_PORT 7000
#define DNS_PORT 8053
#define DNS6_PORT 8054
#define WHOIS_PORT 8043
#define TLS_PORT 8443

// Runs `command` with sh(1), saying so: its exit code, or -1.
static int run(const char *command) {
	printf("nettooltest: $ %s\n", command);
	fflush(stdout);
	int status = system(command);
	return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

// Runs `command` with sh(1) and keeps what it writes, its standard error
// too, in `buffer`: its exit code, or -1.
static int output(const char *command, char *buffer, size_t size) {
	char line[1024];
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
	printf("nettooltest: `%s` never said \"%s\"; it said: %s\n", command, needle, buffer);
	return 0;
}

// Whether `command` exits with `code` and what it writes holds `needle`,
// saying what it wrote.
static int says_with(const char *command, int code, const char *needle) {
	static char buffer[16384];
	int got = output(command, buffer, sizeof(buffer));
	printf("nettooltest: $ %s\n%s", command, buffer);
	if(got != code || !strstr(buffer, needle)) {
		printf("nettooltest: `%s` (exit %d, not %d) did not say \"%s\"\n", command, got, code,
				needle);
		return 0;
	}
	return 1;
}

// Whether `command` succeeds and what it writes holds `needle`.
static int says(const char *command, const char *needle) {
	return says_with(command, 0, needle);
}

// Whether what `command` writes leaves out `needle`.
static int never_says(const char *command, const char *needle) {
	static char buffer[16384];
	output(command, buffer, sizeof(buffer));
	if(strstr(buffer, needle)) {
		printf("nettooltest: `%s` said \"%s\": %s\n", command, needle, buffer);
		return 0;
	}
	return 1;
}

// Writes `text` into `path`.
static void write_file(const char *path, const char *text) {
	FILE *file = fopen(path, "w");
	CHECK(file && fputs(text, file) >= 0 && fclose(file) == 0);
}

// A socket of `type` bound to every address of `family` at `port`; a TCP
// one listening. IPv6's takes IPv4's connections too.
static int bound(int family, int type, unsigned short port) {
	int fd = socket(family, type, 0);
	CHECK(fd >= 0);
	int yes = 1;
	setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));
	struct sockaddr_storage at;
	memset(&at, 0, sizeof(at));
	socklen_t length;
	if(family == AF_INET6) {
		struct sockaddr_in6 *six = (struct sockaddr_in6 *)&at;
		six->sin6_family = AF_INET6;
		six->sin6_port = htons(port);
		six->sin6_addr = in6addr_any;
		length = sizeof(*six);
	} else {
		struct sockaddr_in *four = (struct sockaddr_in *)&at;
		four->sin_family = AF_INET;
		four->sin_port = htons(port);
		four->sin_addr.s_addr = htonl(INADDR_ANY);
		length = sizeof(*four);
	}
	CHECK(bind(fd, (struct sockaddr *)&at, length) == 0);
	if(type == SOCK_STREAM)
		CHECK(listen(fd, 8) == 0);
	return fd;
}

// --- b's name server --------------------------------------------------

// A DNS message being built.
struct message {
	unsigned char bytes[4096];
	size_t length;
};

static void put8(struct message *m, unsigned value) {
	if(m->length < sizeof(m->bytes))
		m->bytes[m->length++] = (unsigned char)value;
}

static void put16(struct message *m, unsigned value) {
	put8(m, value >> 8);
	put8(m, value & 0xff);
}

static void put32(struct message *m, unsigned long value) {
	put16(m, (unsigned)(value >> 16));
	put16(m, (unsigned)(value & 0xffff));
}

// Puts `name`, dotted, in labels.
static void put_name(struct message *m, const char *name) {
	while(*name) {
		const char *dot = strchr(name, '.');
		size_t length = dot ? (size_t)(dot - name) : strlen(name);
		put8(m, (unsigned)length);
		for(size_t i = 0; i < length; i++)
			put8(m, (unsigned char)name[i]);
		name += length + (dot ? 1 : 0);
	}
	put8(m, 0);
}

enum { TYPE_A = 1, TYPE_NS = 2, TYPE_SOA = 6, TYPE_PTR = 12, TYPE_MX = 15, TYPE_TXT = 16,
	TYPE_AAAA = 28, TYPE_OPT = 41, TYPE_ANY = 255 };

// One record of the zone: its owner, type, and data as text: an address,
// a name, `PREFERENCE NAME` for MX, or a string for TXT.
struct record {
	const char *owner;
	unsigned type;
	const char *data;
};

#define BIG "nettooltest: a long string, to make the answer too long for UDP, " \
			"which dig must then ask again over TCP, as BIND's does, and show whole " \
			"..............................................................."

static const struct record ZONE[] = {
		{"tools.test", TYPE_SOA, ""},
		{"tools.test", TYPE_NS, "ns.tools.test"},
		{"tools.test", TYPE_MX, "10 mail.tools.test"},
		{"tools.test", TYPE_TXT, "nettooltest says hello"},
		{"www.tools.test", TYPE_A, "192.168.50.2"},
		{"www.tools.test", TYPE_AAAA, "fd50::2"},
		{"ns.tools.test", TYPE_A, "192.168.50.2"},
		{"mail.tools.test", TYPE_A, "192.168.50.2"},
		{"2.50.168.192.in-addr.arpa", TYPE_PTR, "www.tools.test"},
		{"whois.iana.org", TYPE_A, "192.168.50.2"},
		{"whois.nic.test", TYPE_A, "192.168.50.2"},
		{"whois.registrar.test", TYPE_A, "192.168.50.2"},
		{"whois.arin.net", TYPE_A, "192.168.50.2"},
		{"big.tools.test", TYPE_TXT, BIG},
		{"big.tools.test", TYPE_TXT, BIG " 2"},
		{"big.tools.test", TYPE_TXT, BIG " 3"},
		{"big.tools.test", TYPE_TXT, BIG " 4"},
		{"big.tools.test", TYPE_TXT, BIG " 5"},
		{"big.tools.test", TYPE_TXT, BIG " 6"},
		{"big.tools.test", TYPE_TXT, BIG " 7"},
		{"big.tools.test", TYPE_TXT, BIG " 8"},
		{"big.tools.test", TYPE_TXT, BIG " 9"},
};
enum { ZONE_COUNT = sizeof(ZONE) / sizeof(ZONE[0]) };

// Puts a record of `owner` (a pointer into the question when 0xc00c),
// with its data.
static void put_record(struct message *m, const struct record *r) {
	put16(m, 0xc00c);
	put16(m, r->type);
	put16(m, 1);
	put32(m, 300);
	size_t at = m->length;
	put16(m, 0);
	switch(r->type) {
	case TYPE_A: {
		struct in_addr four;
		inet_pton(AF_INET, r->data, &four);
		for(int i = 0; i < 4; i++)
			put8(m, ((unsigned char *)&four)[i]);
		break;
	}
	case TYPE_AAAA: {
		struct in6_addr six;
		inet_pton(AF_INET6, r->data, &six);
		for(int i = 0; i < 16; i++)
			put8(m, six.s6_addr[i]);
		break;
	}
	case TYPE_NS:
	case TYPE_PTR:
		put_name(m, r->data);
		break;
	case TYPE_MX: {
		const char *space = strchr(r->data, ' ');
		put16(m, (unsigned)atoi(r->data));
		put_name(m, space ? space + 1 : "");
		break;
	}
	case TYPE_TXT: {
		size_t length = strlen(r->data);
		for(size_t done = 0; done < length || !done;) {
			size_t piece = length - done > 255 ? 255 : length - done;
			put8(m, (unsigned)piece);
			for(size_t i = 0; i < piece; i++)
				put8(m, (unsigned char)r->data[done + i]);
			done += piece;
			if(!length)
				break;
		}
		break;
	}
	case TYPE_SOA:
		put_name(m, "ns.tools.test");
		put_name(m, "admin.tools.test");
		put32(m, 2026110201);
		put32(m, 3600);
		put32(m, 600);
		put32(m, 86400);
		put32(m, 300);
		break;
	}
	size_t length = m->length - at - 2;
	m->bytes[at] = (unsigned char)(length >> 8);
	m->bytes[at + 1] = (unsigned char)(length & 0xff);
}

// The zone's SOA record, under its own name, for the authority section.
static void put_soa(struct message *m) {
	put_name(m, "tools.test");
	size_t at = m->length;
	struct record soa = {"tools.test", TYPE_SOA, ""};
	// put_record writes a pointer to the question first: take it back.
	put_record(m, &soa);
	memmove(m->bytes + at, m->bytes + at + 2, m->length - at - 2);
	m->length -= 2;
}

// The answer to `query`, `length` bytes, in `reply`, cut to `most` bytes
// with TC set when it is longer: false for a message not answered.
static int answer_dns(const unsigned char *query, size_t length, size_t most,
		struct message *reply) {
	if(length < 12 || (query[2] & 0x80))
		return 0;
	// The question's name, in lower case and dotted.
	char name[256] = "";
	size_t at = 12, used = 0;
	while(at < length && query[at]) {
		size_t label = query[at++];
		if(label > 63 || at + label > length || used + label + 2 > sizeof(name))
			return 0;
		if(used)
			name[used++] = '.';
		for(size_t i = 0; i < label; i++) {
			unsigned char c = query[at + i];
			name[used++] = (char)(c >= 'A' && c <= 'Z' ? c + 32 : c);
		}
		at += label;
	}
	name[used] = 0;
	if(at + 5 > length)
		return 0;
	at++;
	unsigned qtype = (unsigned)(query[at] << 8 | query[at + 1]);
	size_t question_end = at + 4;
	int edns = (query[10] << 8 | query[11]) > 0;

	reply->length = 0;
	put16(reply, (unsigned)(query[0] << 8 | query[1]));
	int known = 0, answers = 0;
	for(int i = 0; i < ZONE_COUNT; i++)
		if(!strcmp(ZONE[i].owner, name))
			known = 1;
	put16(reply, 0x8400 | (query[2] & 0x01) << 8 | (known ? 0 : 3));
	put16(reply, 1);
	put16(reply, 0);
	put16(reply, 0);
	put16(reply, 0);
	for(size_t i = 12; i < question_end; i++)
		put8(reply, query[i]);
	for(int i = 0; i < ZONE_COUNT; i++) {
		if(strcmp(ZONE[i].owner, name) || (qtype != ZONE[i].type && qtype != TYPE_ANY))
			continue;
		put_record(reply, &ZONE[i]);
		answers++;
	}
	int authority = 0;
	if(!answers) {
		put_soa(reply);
		authority = 1;
	}
	if(edns) {
		put8(reply, 0);
		put16(reply, TYPE_OPT);
		put16(reply, 1232);
		put32(reply, 0);
		put16(reply, 0);
	}
	reply->bytes[7] = (unsigned char)answers;
	reply->bytes[9] = (unsigned char)authority;
	reply->bytes[11] = (unsigned char)edns;
	if(reply->length > most) {
		// Too long: the header and the question alone, truncated.
		reply->length = question_end;
		reply->bytes[2] |= 0x02;
		reply->bytes[7] = reply->bytes[9] = reply->bytes[11] = 0;
	}
	printf("nettooltest: the name server answered %s, type %u, with %d records in %zu bytes\n",
			name, qtype, answers, reply->length);
	return 1;
}

// Answers DNS over UDP on `udp4` and `udp6` and over TCP on `tcp`, until
// killed.
static void name_server(int udp4, int udp6, int tcp) {
	struct pollfd p[3] = {{udp4, POLLIN, 0}, {udp6, POLLIN, 0}, {tcp, POLLIN, 0}};
	static struct message reply;
	for(;;) {
		if(poll(p, 3, -1) < 0)
			continue;
		for(int i = 0; i < 2; i++) {
			if(!(p[i].revents & POLLIN))
				continue;
			unsigned char query[1500];
			struct sockaddr_storage from;
			socklen_t size = sizeof(from);
			ssize_t got = recvfrom(p[i].fd, query, sizeof(query), 0, (struct sockaddr *)&from, &size);
			if(got > 0 && answer_dns(query, (size_t)got, 1232, &reply))
				sendto(p[i].fd, reply.bytes, reply.length, 0, (struct sockaddr *)&from, size);
		}
		if(p[2].revents & POLLIN) {
			int fd = accept(tcp, NULL, NULL);
			if(fd < 0)
				continue;
			unsigned char query[1500];
			unsigned char head[2];
			while(read(fd, head, 2) == 2) {
				size_t length = (size_t)(head[0] << 8 | head[1]), have = 0;
				while(have < length && length <= sizeof(query)) {
					ssize_t got = read(fd, query + have, length - have);
					if(got <= 0)
						break;
					have += (size_t)got;
				}
				if(have != length || !answer_dns(query, length, sizeof(reply.bytes), &reply))
					break;
				unsigned char out[2] = {(unsigned char)(reply.length >> 8),
						(unsigned char)(reply.length & 0xff)};
				if(write(fd, out, 2) != 2 || write(fd, reply.bytes, reply.length) != (ssize_t)reply.length)
					break;
			}
			close(fd);
		}
	}
}

// --- b's WHOIS servers ------------------------------------------------

// What the servers answer, in turn: IANA's, which refers the question to
// the registry; the registry's, which names the registrar's server; and
// the registrar's.
static const char *const WHOIS_ANSWERS[] = {
		("% IANA WHOIS server\r\n"
		 "% for more information on IANA, visit http://www.iana.org\r\n"
		 "\r\n"
		 "refer:        whois.nic.test\r\n"
		 "\r\n"
		 "domain:       TEST\r\n"),
		("   Domain Name: EXAMPLE.TEST\r\n"
		 "   Registrar WHOIS Server: whois.registrar.test\r\n"
		 "   Registrar: The nettooltest registrar\r\n"),
		("Domain Name: EXAMPLE.TEST\r\n"
		 "Registrant Organization: The nettooltest registrant\r\n"),
};

// Where the servers write each question they were asked, a line each.
#define WHOIS_ASKED "/tmp/whois-asked"

// Answers each WHOIS connection to `server`, until killed.
static void whois_server(int server) {
	for(unsigned turn = 0;; turn++) {
		int fd = accept(server, NULL, NULL);
		if(fd < 0) {
			turn--;
			continue;
		}
		char query[256];
		size_t have = 0;
		while(have < sizeof(query) - 1) {
			ssize_t got = read(fd, query + have, 1);
			if(got <= 0 || query[have] == '\n')
				break;
			have++;
		}
		query[have] = 0;
		query[strcspn(query, "\r")] = 0;
		FILE *asked = fopen(WHOIS_ASKED, "a");
		if(asked) {
			fprintf(asked, "%s\n", query);
			fclose(asked);
		}
		const char *answer = WHOIS_ANSWERS[turn % 3];
		printf("nettooltest: the WHOIS server answered '%s' as server %u\n", query, turn % 3);
		fflush(stdout);
		(void)!write(fd, answer, strlen(answer));
		close(fd);
	}
}

// --- set up -----------------------------------------------------------

// The machine's card on the cable, at 192.168.50.N and fd50::N.
static void set_up(int n) {
	char text[256];
	snprintf(text, sizeof(text), "[net0]\ndhcp = no\naddress = 192.168.50.%d/24\naddress = fd50::%d/64\n",
			n, n);
	write_file("/system/settings/network", text);
	CHECK(run("service netd reload") == 0);
	snprintf(text, sizeof(text), "inet6 fd50::%d/64 scope global", n);
	CHECK(wait_for("ip -6 address show dev net0", text, 60));
	CHECK(run("ip address show") == 0);
}

// Machine a's connection to machine b, a stream each way.
static FILE *control_in, *control_out;

// Asks b to do `command`: whether it answered `wanted` (a prefix).
static int ask(const char *command, const char *wanted) {
	printf("nettooltest: asking b: %s\n", command);
	fflush(stdout);
	fprintf(control_out, "%s\n", command);
	fflush(control_out);
	char line[1024];
	if(!fgets(line, sizeof(line), control_in)) {
		printf("nettooltest: FAILED: b did not answer `%s`\n", command);
		check_failures++;
		return 0;
	}
	line[strcspn(line, "\n")] = 0;
	printf("nettooltest: b: %s\n", line);
	if(strncmp(line, wanted, strlen(wanted)) != 0) {
		printf("nettooltest: FAILED: b answered `%s` with `%s`, not `%s`\n", command, line, wanted);
		check_failures++;
		return 0;
	}
	return 1;
}

// --- a's parts --------------------------------------------------------

// nic on a's e1000e, which QEMU's link gives at 1000 Mb/s, full duplex,
// and the kernel's files it reads.
static void nic(void) {
	CHECK(says("cat /kernel/class/net/net0/speed", "1000"));
	CHECK(says("cat /kernel/class/net/net0/duplex", "full"));
	CHECK(says("cat /kernel/class/net/net0/card", "driver radix-intel-net\nmodel Intel 82574L\nbus 0000:"));
	CHECK(says("cat /kernel/class/net/lo/speed", "-1"));
	CHECK(says("nic net0", "Settings for net0:\n\tSpeed: 1000Mb/s\n\tDuplex: Full\n\tLink detected: yes\n"));
	CHECK(says("nic -i net0", "driver: radix-intel-net\n"));
	CHECK(says("nic -i net0", "bus-info: 0000:"));
	CHECK(says("nic -i net0", "model: Intel 82574L\n"));
	CHECK(says("nic -g net0", "Current hardware settings:\nRX:\t\t"));
	CHECK(says("nic -k net0", "tcp-segmentation-offload: off [fixed]\n"));
	CHECK(says("nic -k lo", "rx-checksumming: on [fixed]\n"));
	CHECK(says("nic -S net0", "NIC statistics:\n     rx_bytes: "));
	CHECK(says("nic -S net0", "card_rx_dropped: "));
	CHECK(says("nic -P net0", "Permanent address: 52:54:00:12:34:a0\n"));
	CHECK(says("nic", "radix-intel-net"));
	CHECK(says("nic lo", "Settings for lo:\n\tLink detected: yes\n"));
	CHECK(says_with("nic -s net0 speed 100", 1, "negotiates"));
	CHECK(says_with("nic -g lo", 1, "Operation not supported"));
	CHECK(says_with("nic eth9", 1, "no such interface"));
	printf("nettooltest: nic showed the e1000e's link, driver, rings, offloads and counts\n");
}

// nc both ways: TCP and UDP, by IPv4 and IPv6, and a scan of b's ports.
static void nc(void) {
	CHECK(ask("start nc -d -l 7101 > /tmp/from-a", "started"));
	CHECK(run("echo nettooltest-a-to-b | nc -N 192.168.50.2 7101") == 0);
	CHECK(ask("run cat /tmp/from-a", "ok 0 nettooltest-a-to-b"));
	CHECK(run("nc -d -l 7102 > /tmp/from-b &") == 0);
	check_sleep_ms(1000);
	CHECK(ask("run echo nettooltest-b-to-a | nc -N 192.168.50.1 7102", "ok 0"));
	CHECK(wait_for("cat /tmp/from-b", "nettooltest-b-to-a", 10));
	CHECK(ask("start nc -6 -d -l 7104 > /tmp/from-a6", "started"));
	CHECK(run("echo nettooltest-over-ipv6 | nc -N fd50::2 7104") == 0);
	CHECK(ask("run cat /tmp/from-a6", "ok 0 nettooltest-over-ipv6"));
	CHECK(run("nc -d -u -l 7103 > /tmp/from-b-udp &") == 0);
	check_sleep_ms(1000);
	CHECK(ask("run echo nettooltest-udp | nc -u -w 1 192.168.50.1 7103", "ok 0"));
	CHECK(wait_for("cat /tmp/from-b-udp", "nettooltest-udp", 10));
	// nc's UDP listener peeks at the first datagram for its sender
	// (MSG_PEEK), here on an IPv6 socket.
	CHECK(ask("start nc -6 -d -u -l 7106 > /tmp/from-a-udp6", "started"));
	CHECK(run("echo nettooltest-udp-over-ipv6 | nc -u -w 1 fd50::2 7106") == 0);
	CHECK(ask("run for i in 1 2 3 4 5 6 7 8 9 10; do grep -q udp-over /tmp/from-a-udp6 && break; "
			  "sleep 1; done; cat /tmp/from-a-udp6",
			"ok 0 nettooltest-udp-over-ipv6"));
	CHECK(ask("start nc -d -l 7105", "started"));
	CHECK(says("nc -z -v -w 3 192.168.50.2 7105", "succeeded"));
	CHECK(says_with("nc -z -w 3 192.168.50.2 7199", 1, ""));
	printf("nettooltest: nc carried TCP and UDP both ways, by IPv4 and IPv6, and found b's open port\n");
}

// iperf3 against b's server: TCP, UDP, IPv6, b sending, and JSON.
static void iperf3(void) {
	CHECK(ask("start iperf3 -s -p 5201", "started"));
	CHECK(says("iperf3 -c 192.168.50.2 -p 5201 -t 2", "receiver"));
	CHECK(says("iperf3 -c 192.168.50.2 -p 5201 -t 1 -u -b 20M", "Jitter"));
	CHECK(says("iperf3 -c fd50::2 -p 5201 -t 1 -R", "Reverse mode"));
	CHECK(says("iperf3 -c 192.168.50.2 -p 5201 -t 1 -J", "\"sum_received\""));
	printf("nettooltest: iperf3 measured TCP and UDP to b, by IPv4 and IPv6, both ways\n");
}

// dig and host against b's name server, and host through resolved,
// which a's settings point at b's.
static void names(void) {
	CHECK(says("dig @192.168.50.2 -p 8053 www.tools.test", "status: NOERROR"));
	CHECK(says("dig @192.168.50.2 -p 8053 www.tools.test", "www.tools.test.\t\t300\tIN\tA\t192.168.50.2"));
	CHECK(says("dig @192.168.50.2 -p 8053 www.tools.test", ";; SERVER: 192.168.50.2#8053(192.168.50.2) (UDP)"));
	CHECK(says("dig @192.168.50.2 -p 8053 +short www.tools.test AAAA", "fd50::2"));
	CHECK(says("dig @192.168.50.2 -p 8053 +short tools.test MX", "10 mail.tools.test."));
	CHECK(says("dig @192.168.50.2 -p 8053 +short -t TXT tools.test", "\"nettooltest says hello\""));
	CHECK(says("dig @192.168.50.2 -p 8053 +short -x 192.168.50.2", "www.tools.test."));
	CHECK(says("dig @192.168.50.2 -p 8053 +noall +answer tools.test SOA",
			"ns.tools.test. admin.tools.test. 2026110201 3600 600 86400 300"));
	CHECK(says("dig @192.168.50.2 -p 8053 missing.tools.test", "status: NXDOMAIN"));
	CHECK(says("dig @192.168.50.2 -p 8053 big.tools.test TXT", ";; Truncated, retrying in TCP mode."));
	CHECK(says("dig @192.168.50.2 -p 8053 big.tools.test TXT", "ANSWER: 9,"));
	CHECK(says("dig @192.168.50.2 -p 8053 +tcp www.tools.test", "(TCP)"));
	CHECK(says("dig @fd50::2 -p 8054 +short www.tools.test", "192.168.50.2"));
	CHECK(says_with("dig @192.168.50.9 -p 8053 +time=1 +tries=1 www.tools.test", 9, "timed out"));
	CHECK(says("host -p 8053 www.tools.test 192.168.50.2", "www.tools.test has address 192.168.50.2\n"));
	CHECK(says("host -p 8053 www.tools.test 192.168.50.2", "www.tools.test has IPv6 address fd50::2\n"));
	CHECK(says("host -p 8053 -t MX tools.test 192.168.50.2", "tools.test mail is handled by 10 mail.tools.test."));
	CHECK(says("host -p 8053 192.168.50.2 192.168.50.2", "domain name pointer www.tools.test."));
	CHECK(says_with("host -p 8053 missing.tools.test 192.168.50.2", 1, "not found: 3(NXDOMAIN)"));
	// Through resolved, which asks b's server.
	write_file("/system/settings/resolved", "servers = 192.168.50.2:8053\ndns-over-tls = no\n");
	CHECK(run("service resolved reload") == 0);
	CHECK(wait_for("cat /system/live/resolved/status", "Server: 192.168.50.2:8053 (from the settings)", 30));
	CHECK(says("host www.tools.test", "www.tools.test has address 192.168.50.2"));
	CHECK(says("dig +short www.tools.test", "192.168.50.2"));
	printf("nettooltest: dig and host asked b's name server, and through resolved\n");
}

// whois from IANA's server through the referrals, the names of their
// servers found through resolved; one server alone; and ARIN's asked for
// networks.
static void whois(void) {
	const char *full = "whois -p 8043 example.test";
	CHECK(says(full, "% whois.iana.org:\n"));
	CHECK(ask("run grep -c . /tmp/whois-asked", "ok 0 3"));
	CHECK(ask("run head -n 1 /tmp/whois-asked", "ok 0 example.test"));
	CHECK(ask("run rm /tmp/whois-asked", "ok 0"));
	// The servers answer in turn: each run asks three.
	CHECK(says(full, "% whois.nic.test:\n"));
	CHECK(says(full, "% whois.registrar.test:\n"));
	CHECK(says(full, "Registrant Organization: The nettooltest registrant"));
	CHECK(says("whois -h 192.168.50.2 -p 8043 example.test", "refer:        whois.nic.test"));
	CHECK(never_says("whois -h 192.168.50.2 -p 8043 example.test", "% whois.nic.test:"));
	CHECK(ask("run rm /tmp/whois-asked", "ok 0"));
	CHECK(run("whois -h whois.arin.net -p 8043 192.0.2.1") == 0);
	CHECK(ask("run cat /tmp/whois-asked", "ok 0 n + 192.0.2.1"));
	CHECK(says_with("whois -h 192.168.50.9 -p 8043 example.test", 2, "192.168.50.9"));
	printf("nettooltest: whois followed IANA's referrals to the registrar, and asked ARIN for networks\n");
}

// openssl's keys, certificates and digests, and b's s_client reaching
// a's s_server over TLS 1.3.
static void openssl(void) {
	CHECK(says("openssl version", "OpenSSL 3.5.4"));
	CHECK(says("openssl version -d", "OPENSSLDIR: \"/system/settings/ssl\""));
	CHECK(says("printf abc | openssl dgst -sha256",
			"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
	CHECK(says("openssl rand -hex 16 | wc -c", "33"));
	CHECK(run("openssl genpkey -algorithm ed25519 -out /tmp/key.pem") == 0);
	CHECK(run("openssl req -new -x509 -key /tmp/key.pem -subj /CN=a.tools.test -days 1 "
			  "-out /tmp/cert.pem") == 0);
	CHECK(says("openssl x509 -in /tmp/cert.pem -noout -subject", "subject=CN=a.tools.test"));
	CHECK(says("openssl verify -CAfile /tmp/cert.pem /tmp/cert.pem", "/tmp/cert.pem: OK"));
	CHECK(says("openssl ecparam -name prime256v1 -genkey -noout | openssl ec -noout -text",
			"ASN1 OID: prime256v1"));
	CHECK(says("echo nettooltest | openssl enc -aes-256-cbc -pbkdf2 -pass pass:x -a | "
			   "openssl enc -d -aes-256-cbc -pbkdf2 -pass pass:x -a",
			"nettooltest"));
	CHECK(says("openssl x509 -in /system/settings/ssl/cert.pem -noout -subject", "subject="));
	CHECK(run("openssl s_server -accept 8443 -cert /tmp/cert.pem -key /tmp/key.pem -www "
			  "> /tmp/s_server.log 2>&1 &") == 0);
	check_sleep_ms(2000);
	CHECK(ask("run openssl s_client -connect 192.168.50.1:8443 -servername a.tools.test "
			  "< /dev/null 2>&1 | grep -c 'subject=CN=a.tools.test'",
			"ok 0 1"));
	CHECK(ask("run openssl s_client -connect 192.168.50.1:8443 < /dev/null 2>&1 | grep -o 'New, TLSv1.3'",
			"ok 0 New, TLSv1.3"));
	printf("nettooltest: openssl made keys and a certificate, and b's s_client reached a's s_server over TLS 1.3\n");
}

static int a(void) {
	set_up(1);
	RUN(nic);
	int server = bound(AF_INET6, SOCK_STREAM, CONTROL_PORT);
	printf("nettooltest: waiting for b on port %d\n", CONTROL_PORT);
	fflush(stdout);
	struct pollfd p = {server, POLLIN, 0};
	CHECK(poll(&p, 1, 300000) == 1);
	int fd = accept(server, NULL, NULL);
	CHECK(fd >= 0);
	close(server);
	if(fd >= 0) {
		control_in = fdopen(fd, "r");
		control_out = fdopen(dup(fd), "w");
		RUN(nc);
		RUN(iperf3);
		RUN(names);
		RUN(whois);
		RUN(openssl);
		CHECK(ask("done", "bye"));
		fclose(control_out);
		fclose(control_in);
	}
	return check_end();
}

// Does what a asks, from `from`, and answers it on `to`.
static void follow(FILE *from, FILE *to) {
	char line[1024];
	while(fgets(line, sizeof(line), from)) {
		line[strcspn(line, "\n")] = 0;
		printf("nettooltest: a asks: %s\n", line);
		char reply[1200];
		if(!strncmp(line, "run ", 4)) {
			static char said[16384];
			int code = output(line + 4, said, sizeof(said));
			printf("%s", said);
			said[strcspn(said, "\n")] = 0;
			snprintf(reply, sizeof(reply), "ok %d %s", code, said);
		} else if(!strncmp(line, "start ", 6)) {
			char command[1100];
			snprintf(command, sizeof(command), "%s &", line + 6);
			CHECK(run(command) == 0);
			check_sleep_ms(1000);
			snprintf(reply, sizeof(reply), "started");
		} else if(!strcmp(line, "done")) {
			fprintf(to, "bye\n");
			fflush(to);
			return;
		} else {
			snprintf(reply, sizeof(reply), "what is `%s`?", line);
		}
		printf("nettooltest: answering: %s\n", reply);
		fprintf(to, "%s\n", reply);
		fflush(to);
	}
	printf("nettooltest: FAILED: a went before it was done\n");
	check_failures++;
}

// Connects to a's control port, waiting for a to listen: the socket, or
// -1.
static int reach_a(void) {
	for(int i = 0; i < 300; i++) {
		int fd = socket(AF_INET, SOCK_STREAM, 0);
		struct sockaddr_in to;
		memset(&to, 0, sizeof(to));
		to.sin_family = AF_INET;
		to.sin_port = htons(CONTROL_PORT);
		inet_pton(AF_INET, "192.168.50.1", &to.sin_addr);
		if(fd >= 0 && connect(fd, (struct sockaddr *)&to, sizeof(to)) == 0)
			return fd;
		if(fd >= 0)
			close(fd);
		check_sleep_ms(1000);
	}
	return -1;
}

static int b(void) {
	set_up(2);
	// nic on b's virtio card, which says whether its link is up and
	// nothing of its speed.
	CHECK(says("nic net0", "\tSpeed: Unknown!\n\tDuplex: Unknown! (255)\n\tLink detected: yes\n"));
	CHECK(says("nic -i net0", "driver: radix-virtio\n"));
	CHECK(says("cat /kernel/class/net/net0/speed", "-1"));
	printf("nettooltest: nic showed the virtio card's link, which says no speed\n");
	pid_t names = check_fork();
	if(names == 0)
		name_server(bound(AF_INET, SOCK_DGRAM, DNS_PORT), bound(AF_INET6, SOCK_DGRAM, DNS6_PORT),
				bound(AF_INET6, SOCK_STREAM, DNS_PORT));
	pid_t registries = check_fork();
	if(registries == 0)
		whois_server(bound(AF_INET6, SOCK_STREAM, WHOIS_PORT));
	int fd = reach_a();
	if(fd < 0)
		printf("nettooltest: FAILED: cannot reach a on port %d\n", CONTROL_PORT);
	CHECK(fd >= 0);
	if(fd >= 0) {
		printf("nettooltest: connected to a\n");
		FILE *from = fdopen(fd, "r");
		FILE *to = fdopen(dup(fd), "w");
		follow(from, to);
		fclose(to);
		fclose(from);
	}
	kill(names, SIGKILL);
	check_wait(names);
	kill(registries, SIGKILL);
	check_wait(registries);
	return check_end();
}

int main(int argc, char **argv) {
	check_start("nettooltest");
	if(argc == 2 && !strcmp(argv[1], "a"))
		return a();
	if(argc == 2 && !strcmp(argv[1], "b"))
		return b();
	printf("usage: nettooltest a | b\n");
	return 2;
}
