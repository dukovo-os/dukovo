// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The network-trace boot test's program (ADR 0345): the routers a
// traceroute finds, played on a tunnel. It makes the tunnel trace0,
// 10.77.0.2/16 and fd77::2/64, and opens it, so that what the machine
// sends there comes to its router thread, which answers as four hops to
// 10.77.9.9 and fd77::99 would:
//
// - hop 1, 10.77.1.1 and fd77:1::1, says "time exceeded", quoting the
//   whole packet, as Linux's routers do;
// - hop 2 says nothing;
// - hop 3, 10.77.3.1 and fd77:3::1, says "time exceeded" too, quoting
//   only the IP header and 8 bytes after it, as RFC 792's routers do;
// - 10.77.9.9 and fd77::99 answer an echo request and say "port
//   unreachable" for a UDP datagram.
//
// And 10.77.0.53 is a name server: hop-1.trace.test, hop-3.trace.test
// and far.trace.test are those addresses' names, far.trace.test is
// 10.77.9.9, and origin.asn.cymru.com's TXT records give AS64500.
//
// It checks that sockets learn of these errors as on Linux, by
// IP_RECVERR's queue and as ECONNREFUSED, the hop limits of datagrams
// sent together kept apart; then runs traceroute, traceroute6, mtr and
// ping against the hops and checks what they print.

#include <arpa/inet.h>
#include <fcntl.h>
#include <linux/errqueue.h>
#include <netinet/in.h>
#include <poll.h>
#include <pthread.h>
#include <radix/radix.h>
#include <sys/socket.h>
#include <sys/stat.h>

#include "check.h"

#define TUNNEL "trace0"
#define FAR "10.77.9.9"
#define FAR6 "fd77::99"
#define NAMESERVER "10.77.0.53"
#define PORT 33434
#define RESOLV "/system/live/resolved/resolv.conf"

// The tunnel's file, and whether the router thread is to stop.
static int tunnel = -1;
static volatile int stopping;

static uint32_t add_up(const uint8_t *p, size_t n, uint32_t sum) {
	for(size_t i = 0; i + 1 < n; i += 2)
		sum += (uint32_t)p[i] << 8 | p[i + 1];
	if(n & 1)
		sum += (uint32_t)p[n - 1] << 8;
	return sum;
}

static uint16_t folded(uint32_t sum) {
	while(sum >> 16)
		sum = (sum & 0xffff) + (sum >> 16);
	return (uint16_t)~sum;
}

static void put16(uint8_t *p, uint16_t value) {
	p[0] = value >> 8;
	p[1] = value & 0xff;
}

static uint16_t get16(const uint8_t *p) {
	return (uint16_t)(p[0] << 8 | p[1]);
}

// An IPv4 packet from `from` to `to` of `protocol` around the `length`
// bytes already at out + 20: its length.
static size_t ipv4(uint8_t *out, const char *from, const uint8_t *to, uint8_t protocol,
		size_t length) {
	memset(out, 0, 20);
	out[0] = 0x45;
	put16(out + 2, (uint16_t)(20 + length));
	out[8] = 64;
	out[9] = protocol;
	inet_pton(AF_INET, from, out + 12);
	memcpy(out + 16, to, 4);
	put16(out + 10, folded(add_up(out, 20, 0)));
	return 20 + length;
}

// An IPv6 packet's header, from `from` to `to`, with `next` and the
// `length` bytes already at out + 40, and the checksum at out + 40 +
// `checksum` of what it carries: its length.
static size_t ipv6(uint8_t *out, const uint8_t *from, const uint8_t *to, uint8_t next,
		size_t length, size_t checksum) {
	memset(out, 0, 40);
	out[0] = 0x60;
	put16(out + 4, (uint16_t)length);
	out[6] = next;
	out[7] = 64;
	memcpy(out + 8, from, 16);
	memcpy(out + 24, to, 16);
	put16(out + 40 + checksum, 0);
	uint8_t pseudo[40] = {0};
	memcpy(pseudo, from, 16);
	memcpy(pseudo + 16, to, 16);
	pseudo[34] = length >> 8;
	pseudo[35] = length & 0xff;
	pseudo[39] = next;
	put16(out + 40 + checksum, folded(add_up(out + 40, length, add_up(pseudo, 40, 0))));
	return 40 + length;
}

// An ICMP error of `type` and `code` from `from` about `packet`, quoting
// `quote` bytes of it.
static void icmp_error(uint8_t type, uint8_t code, const char *from, const uint8_t *packet,
		size_t quote) {
	uint8_t out[600];
	if(quote > 548)
		quote = 548;
	uint8_t *icmp = out + 20;
	memset(icmp, 0, 8);
	icmp[0] = type;
	icmp[1] = code;
	memcpy(icmp + 8, packet, quote);
	put16(icmp + 2, folded(add_up(icmp, 8 + quote, 0)));
	size_t length = ipv4(out, from, packet + 12, 1, 8 + quote);
	CHECK(write(tunnel, out, length) == (ssize_t)length);
}

// An ICMPv6 error of `type` and `code` from `from` about `packet`, of
// `length` bytes, quoting it whole.
static void icmp6_error(uint8_t type, uint8_t code, const char *from, const uint8_t *packet,
		size_t length) {
	uint8_t out[1280];
	if(length > sizeof(out) - 48)
		length = sizeof(out) - 48;
	uint8_t *icmp = out + 40;
	memset(icmp, 0, 8);
	icmp[0] = type;
	icmp[1] = code;
	memcpy(icmp + 8, packet, length);
	uint8_t source[16];
	inet_pton(AF_INET6, from, source);
	size_t total = ipv6(out, source, packet + 8, 58, 8 + length, 2);
	CHECK(write(tunnel, out, total) == (ssize_t)total);
}

// The dotted name at `at` in the DNS message `msg` of `size` bytes, in
// lower case, into `name`: the offset past it, or 0.
static size_t dns_name(const uint8_t *msg, size_t size, size_t at, char *name, size_t room) {
	size_t length = 0;
	while(at < size && msg[at]) {
		size_t label = msg[at++];
		if(label > 63 || at + label > size || length + label + 2 > room)
			return 0;
		if(length)
			name[length++] = '.';
		for(size_t i = 0; i < label; i++) {
			char c = (char)msg[at + i];
			name[length++] = c >= 'A' && c <= 'Z' ? c - 'A' + 'a' : c;
		}
		at += label;
	}
	name[length] = 0;
	return at < size ? at + 1 : 0;
}

// Writes `name` as a DNS name at `out`: its length.
static size_t put_name(uint8_t *out, const char *name) {
	size_t at = 0;
	while(*name) {
		const char *dot = strchr(name, '.');
		size_t label = dot ? (size_t)(dot - name) : strlen(name);
		out[at++] = (uint8_t)label;
		memcpy(out + at, name, label);
		at += label;
		name += label + (dot ? 1 : 0);
	}
	out[at++] = 0;
	return at;
}

static const char *const NAMES[][2] = {
	{"1.1.77.10.in-addr.arpa", "hop-1.trace.test"},
	{"1.3.77.10.in-addr.arpa", "hop-3.trace.test"},
	{"9.9.77.10.in-addr.arpa", "far.trace.test"},
};

#define ORIGIN ".origin.asn.cymru.com"
#define AS_TEXT "64500 | 10.77.0.0/16 | ZZ | test | 2026-11-02"

// Answers the DNS query in `packet`, a UDP datagram to the name server.
static void answer_query(const uint8_t *packet, size_t length) {
	size_t header = (packet[0] & 0xf) * 4;
	const uint8_t *query = packet + header + 8;
	size_t size = length - header - 8;
	if(size < 12 || get16(query + 4) != 1)
		return;
	char name[256];
	size_t end = dns_name(query, size, 12, name, sizeof(name));
	if(!end || end + 4 > size)
		return;
	uint16_t type = get16(query + end);
	end += 4;

	uint8_t out[600];
	uint8_t *udp = out + 20;
	uint8_t *msg = udp + 8;
	memcpy(msg, query, end);
	put16(msg + 2, 0x8180);
	put16(msg + 6, 0);
	put16(msg + 8, 0);
	put16(msg + 10, 0);
	size_t at = end;
	uint8_t data[128];
	size_t data_length = 0;
	int found = 0;
	for(size_t i = 0; i < sizeof(NAMES) / sizeof(NAMES[0]); i++) {
		if(!strcmp(name, NAMES[i][0])) {
			found = 1;
			if(type == 12)
				data_length = put_name(data, NAMES[i][1]);
		}
	}
	if(!strcmp(name, "far.trace.test")) {
		found = 1;
		if(type == 1) {
			inet_pton(AF_INET, FAR, data);
			data_length = 4;
		}
	}
	size_t name_length = strlen(name);
	if(name_length > strlen(ORIGIN) && !strcmp(name + name_length - strlen(ORIGIN), ORIGIN)) {
		found = 1;
		if(type == 16) {
			data[0] = (uint8_t)strlen(AS_TEXT);
			memcpy(data + 1, AS_TEXT, strlen(AS_TEXT));
			data_length = 1 + strlen(AS_TEXT);
		}
	}
	if(!found) {
		put16(msg + 2, 0x8183);
	} else if(data_length) {
		put16(msg + 6, 1);
		put16(msg + at, 0xc00c);
		put16(msg + at + 2, type);
		put16(msg + at + 4, 1);
		put16(msg + at + 6, 0);
		put16(msg + at + 8, 60);
		put16(msg + at + 10, (uint16_t)data_length);
		memcpy(msg + at + 12, data, data_length);
		at += 12 + data_length;
	}
	put16(udp, 53);
	memcpy(udp + 2, packet + header, 2);
	put16(udp + 4, (uint16_t)(8 + at));
	put16(udp + 6, 0);
	uint8_t pseudo[12] = {0};
	inet_pton(AF_INET, NAMESERVER, pseudo);
	memcpy(pseudo + 4, packet + 12, 4);
	pseudo[9] = 17;
	put16(pseudo + 10, (uint16_t)(8 + at));
	uint16_t sum = folded(add_up(udp, 8 + at, add_up(pseudo, 12, 0)));
	put16(udp + 6, sum ? sum : 0xffff);
	size_t total = ipv4(out, NAMESERVER, packet + 12, 17, 8 + at);
	CHECK(write(tunnel, out, total) == (ssize_t)total);
}

// What hop the router plays for a packet sent with `ttl`: 1, 2 or 3, or
// 4 for the far machine.
static int hop_of(int ttl) {
	return ttl < 4 ? ttl : 4;
}

static void route_ipv4(const uint8_t *packet, size_t length) {
	if(length < 28 || (packet[0] >> 4) != 4)
		return;
	size_t header = (packet[0] & 0xf) * 4;
	uint8_t protocol = packet[9];
	char to[INET_ADDRSTRLEN];
	inet_ntop(AF_INET, packet + 16, to, sizeof(to));
	if(protocol == 17 && !strcmp(to, NAMESERVER) && get16(packet + header + 2) == 53) {
		answer_query(packet, length);
		return;
	}
	if(strcmp(to, FAR))
		return;
	int echo = protocol == 1 && packet[header] == 8;
	if(protocol != 17 && !echo)
		return;
	int hop = hop_of(packet[8]);
	switch(hop) {
	case 1:
		icmp_error(11, 0, "10.77.1.1", packet, length);
		return;
	case 2:
		return;
	case 3:
		icmp_error(11, 0, "10.77.3.1", packet, header + 8);
		return;
	}
	if(!echo) {
		icmp_error(3, 3, FAR, packet, length);
		return;
	}
	// The echo reply: the request, turned round.
	uint8_t out[1600];
	size_t data = length - header;
	if(data > sizeof(out) - 20)
		return;
	memcpy(out + 20, packet + header, data);
	out[20] = 0;
	put16(out + 22, 0);
	put16(out + 22, folded(add_up(out + 20, data, 0)));
	size_t total = ipv4(out, FAR, packet + 12, 1, data);
	CHECK(write(tunnel, out, total) == (ssize_t)total);
}

static void route_ipv6(const uint8_t *packet, size_t length) {
	if(length < 48)
		return;
	uint8_t far[16];
	inet_pton(AF_INET6, FAR6, far);
	if(memcmp(packet + 24, far, 16))
		return;
	uint8_t next = packet[6];
	int echo = next == 58 && packet[40] == 128;
	if(next != 17 && !echo)
		return;
	int hop = hop_of(packet[7]);
	switch(hop) {
	case 1:
		icmp6_error(3, 0, "fd77:1::1", packet, length);
		return;
	case 2:
		return;
	case 3:
		icmp6_error(3, 0, "fd77:3::1", packet, length);
		return;
	}
	if(!echo) {
		icmp6_error(1, 4, FAR6, packet, length);
		return;
	}
	uint8_t out[1600];
	size_t data = length - 40;
	if(data > sizeof(out) - 40)
		return;
	memcpy(out + 40, packet + 40, data);
	out[40] = 129;
	size_t total = ipv6(out, far, packet + 8, 58, data, 2);
	CHECK(write(tunnel, out, total) == (ssize_t)total);
}

// The router: answers what the machine sends to the tunnel until told
// to stop.
static void *router(void *unused) {
	(void)unused;
	uint8_t packet[2048];
	while(!stopping) {
		struct pollfd p = {tunnel, POLLIN, 0};
		if(poll(&p, 1, 100) <= 0)
			continue;
		ssize_t got = read(tunnel, packet, sizeof(packet));
		if(got <= 0)
			continue;
		if((packet[0] >> 4) == 6)
			route_ipv6(packet, (size_t)got);
		else
			route_ipv4(packet, (size_t)got);
	}
	return NULL;
}

// Makes the tunnel, opens it, and gives it its addresses.
static void make_tunnel(void) {
	struct radix_network_add_link_results made;
	CHECK(radix_network_add_link(RADIX_STARTUP_SLOT_NETWORK, RADIX_LINK_KIND_TUNNEL, 0, 0, TUNNEL,
			strlen(TUNNEL), &made) == 0);
	// Into the slot of the lowest free descriptor.
	int fd = open("/devices/null", O_RDONLY);
	CHECK(fd >= 0);
	close(fd);
	CHECK(radix_network_open_tunnel(RADIX_STARTUP_SLOT_NETWORK, RADIX_STARTUP_SLOT_FD_BASE + fd,
			made.interface, RADIX_PIPE_FLAGS_NON_BLOCKING | RADIX_PIPE_FLAGS_CLOSE_ON_EXEC) == 0);
	tunnel = fd;
	struct radix_interface_address four = {.ip = {10, 77, 0, 2}, .prefix = 16};
	CHECK(radix_network_configure(RADIX_STARTUP_SLOT_NETWORK, made.interface, &four, sizeof(four),
			NULL, 0) == 0);
	struct radix_ipv6_settings settings = {.flags = RADIX_IPV6_FLAGS_ENABLED};
	struct radix_interface_address_ipv6 six = {.prefix = 64};
	inet_pton(AF_INET6, "fd77::2", six.ip);
	CHECK(radix_network_configure_ipv6(RADIX_STARTUP_SLOT_NETWORK, made.interface, &settings,
			sizeof(settings), &six, sizeof(six), NULL, 0) == 0);
	// The name server, as resolved(8) would write it.
	mkdir("/system/live/resolved", 0755);
	FILE *conf = fopen(RESOLV, "w");
	CHECK(conf != NULL);
	if(conf) {
		fprintf(conf, "nameserver " NAMESERVER "\n");
		fclose(conf);
	}
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, router, NULL) == 0);
	CHECK(pthread_detach(thread) == 0);
	// The IPv6 address is tested first; wait for it.
	for(int i = 0; i < 50; i++) {
		int probe = socket(AF_INET6, SOCK_DGRAM, 0);
		struct sockaddr_in6 at = {.sin6_family = AF_INET6};
		inet_pton(AF_INET6, "fd77::2", &at.sin6_addr);
		int bound = bind(probe, (struct sockaddr *)&at, sizeof(at));
		close(probe);
		if(!bound)
			break;
		check_sleep_ms(100);
	}
	printf("tracetest: " TUNNEL " is up, with its router\n");
}

// The error queued on `fd` (struct queued as inet.c's), into `error`,
// with what it quotes into `data`: the length of that, or -1.
struct queued {
	struct sock_extended_err ee;
	union {
		struct sockaddr_in in;
		struct sockaddr_in6 in6;
	} offender;
};

static ssize_t queued_error(int fd, void *data, size_t size, int level, int type,
		struct queued *error) {
	struct pollfd p = {fd, POLLIN, 0};
	if(poll(&p, 1, 5000) != 1 || !(p.revents & POLLERR)) {
		errno = ETIMEDOUT;
		return -1;
	}
	struct iovec iov = {data, size};
	union {
		struct cmsghdr align;
		char space[CMSG_SPACE(sizeof(struct queued))];
	} control;
	struct sockaddr_in6 to;
	struct msghdr hdr;
	memset(&hdr, 0, sizeof(hdr));
	hdr.msg_name = &to;
	hdr.msg_namelen = sizeof(to);
	hdr.msg_iov = &iov;
	hdr.msg_iovlen = 1;
	hdr.msg_control = control.space;
	hdr.msg_controllen = sizeof(control.space);
	ssize_t got = recvmsg(fd, &hdr, MSG_ERRQUEUE);
	if(got < 0)
		return got;
	struct cmsghdr *c = CMSG_FIRSTHDR(&hdr);
	CHECK(c && c->cmsg_level == level && c->cmsg_type == type);
	memset(error, 0, sizeof(*error));
	if(c)
		memcpy(error, CMSG_DATA(c), c->cmsg_len - CMSG_LEN(0));
	return got;
}

static int offender_is(const struct queued *error, const char *address) {
	char text[INET6_ADDRSTRLEN] = "";
	const struct sockaddr *from = SO_EE_OFFENDER(&error->ee);
	if(from->sa_family == AF_INET)
		inet_ntop(AF_INET, &((const struct sockaddr_in *)from)->sin_addr, text, sizeof(text));
	else if(from->sa_family == AF_INET6)
		inet_ntop(AF_INET6, &((const struct sockaddr_in6 *)from)->sin6_addr, text, sizeof(text));
	if(strcmp(text, address))
		printf("tracetest: the error came from %s, not %s\n", text, address);
	return !strcmp(text, address);
}

static struct sockaddr_in far_port(void) {
	struct sockaddr_in to = {.sin_family = AF_INET, .sin_port = htons(PORT)};
	inet_pton(AF_INET, FAR, &to.sin_addr);
	return to;
}

// UDP and ping sockets learn what the routers say, as on Linux.
static void sockets(void) {
	char data[64];
	struct queued error;
	struct sockaddr_in to = far_port();
	int fd = socket(AF_INET, SOCK_DGRAM, 0);
	CHECK(fd >= 0);
	int on = 1;
	CHECK(setsockopt(fd, SOL_IP, IP_RECVERR, &on, sizeof(on)) == 0);
	// Hop 1 quotes all of it: the data comes back.
	int ttl = 1;
	CHECK(setsockopt(fd, IPPROTO_IP, IP_TTL, &ttl, sizeof(ttl)) == 0);
	CHECK(sendto(fd, "probe one", 9, 0, (struct sockaddr *)&to, sizeof(to)) == 9);
	CHECK(queued_error(fd, data, sizeof(data), SOL_IP, IP_RECVERR, &error) == 9);
	CHECK(!memcmp(data, "probe one", 9));
	CHECK(error.ee.ee_errno == EHOSTUNREACH && error.ee.ee_origin == SO_EE_ORIGIN_ICMP);
	CHECK(error.ee.ee_type == 11 && error.ee.ee_code == 0);
	CHECK(offender_is(&error, "10.77.1.1"));
	// Hop 3 quotes 8 bytes past the IP header: the UDP header alone.
	ttl = 3;
	CHECK(setsockopt(fd, IPPROTO_IP, IP_TTL, &ttl, sizeof(ttl)) == 0);
	CHECK(sendto(fd, "probe three", 11, 0, (struct sockaddr *)&to, sizeof(to)) == 11);
	CHECK(queued_error(fd, data, sizeof(data), SOL_IP, IP_RECVERR, &error) == 0);
	CHECK(error.ee.ee_type == 11 && offender_is(&error, "10.77.3.1"));
	// Datagrams sent together keep their own hop limits. A router's
	// answer to one may come before the next is sent: with IP_RECVERR,
	// as on Linux, that send fails with the error, which stays queued,
	// and the datagram is sent again.
	for(ttl = 1; ttl <= 4; ttl++) {
		CHECK(setsockopt(fd, IPPROTO_IP, IP_TTL, &ttl, sizeof(ttl)) == 0);
		ssize_t sent = -1;
		for(int tries = 0; tries < 4 && sent < 0; tries++) {
			sent = sendto(fd, "hop", 3, 0, (struct sockaddr *)&to, sizeof(to));
			if(sent < 0 && errno != EHOSTUNREACH)
				break;
		}
		CHECK(sent == 3);
	}
	CHECK(queued_error(fd, data, sizeof(data), SOL_IP, IP_RECVERR, &error) == 3);
	CHECK(error.ee.ee_type == 11 && offender_is(&error, "10.77.1.1"));
	// Hop 2 says nothing, and 3's quote holds no data.
	CHECK(queued_error(fd, data, sizeof(data), SOL_IP, IP_RECVERR, &error) == 0);
	CHECK(error.ee.ee_type == 11 && offender_is(&error, "10.77.3.1"));
	CHECK(queued_error(fd, data, sizeof(data), SOL_IP, IP_RECVERR, &error) == 3);
	CHECK(error.ee.ee_errno == ECONNREFUSED && error.ee.ee_type == 3 && error.ee.ee_code == 3);
	CHECK(offender_is(&error, FAR));
	CHECK_ERROR(recv(fd, data, sizeof(data), MSG_ERRQUEUE | MSG_DONTWAIT), EAGAIN);
	close(fd);

	// Connected, without IP_RECVERR: "time exceeded" is a soft error,
	// which is not told; "port unreachable" fails the next call.
	fd = socket(AF_INET, SOCK_DGRAM, 0);
	CHECK(fd >= 0 && connect(fd, (struct sockaddr *)&to, sizeof(to)) == 0);
	ttl = 1;
	CHECK(setsockopt(fd, IPPROTO_IP, IP_TTL, &ttl, sizeof(ttl)) == 0);
	CHECK(send(fd, "soft", 4, 0) == 4);
	struct pollfd p = {fd, POLLIN, 0};
	CHECK(poll(&p, 1, 500) == 0);
	ttl = 64;
	CHECK(setsockopt(fd, IPPROTO_IP, IP_TTL, &ttl, sizeof(ttl)) == 0);
	CHECK(send(fd, "hard", 4, 0) == 4);
	CHECK(poll(&p, 1, 5000) == 1 && (p.revents & POLLERR));
	CHECK_ERROR(recv(fd, data, sizeof(data), 0), ECONNREFUSED);
	close(fd);

	// A ping socket's errors quote the request from its ICMP header.
	fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_ICMP);
	CHECK(fd >= 0);
	CHECK(setsockopt(fd, SOL_IP, IP_RECVERR, &on, sizeof(on)) == 0);
	ttl = 1;
	CHECK(setsockopt(fd, IPPROTO_IP, IP_TTL, &ttl, sizeof(ttl)) == 0);
	unsigned char request[16] = {8, 0, 0, 0, 0, 0, 0, 5, 't', 'r', 'a', 'c', 'e', 'm', 'e', '!'};
	struct sockaddr_in ping_to = to;
	ping_to.sin_port = 0;
	CHECK(sendto(fd, request, sizeof(request), 0, (struct sockaddr *)&ping_to, sizeof(ping_to))
			== (ssize_t)sizeof(request));
	unsigned char quoted[64];
	CHECK(queued_error(fd, quoted, sizeof(quoted), SOL_IP, IP_RECVERR, &error) == 16);
	CHECK(quoted[0] == 8 && quoted[7] == 5 && !memcmp(quoted + 8, "traceme!", 8));
	CHECK(error.ee.ee_errno == EHOSTUNREACH && error.ee.ee_type == 11);
	CHECK(offender_is(&error, "10.77.1.1"));
	close(fd);

	// IPv6.
	int six = socket(AF_INET6, SOCK_DGRAM, 0);
	CHECK(six >= 0);
	CHECK(setsockopt(six, SOL_IPV6, IPV6_RECVERR, &on, sizeof(on)) == 0);
	struct sockaddr_in6 to6 = {.sin6_family = AF_INET6, .sin6_port = htons(PORT)};
	inet_pton(AF_INET6, FAR6, &to6.sin6_addr);
	int hops = 1;
	CHECK(setsockopt(six, IPPROTO_IPV6, IPV6_UNICAST_HOPS, &hops, sizeof(hops)) == 0);
	CHECK(sendto(six, "probe6", 6, 0, (struct sockaddr *)&to6, sizeof(to6)) == 6);
	CHECK(queued_error(six, data, sizeof(data), SOL_IPV6, IPV6_RECVERR, &error) == 6);
	CHECK(!memcmp(data, "probe6", 6));
	CHECK(error.ee.ee_errno == EHOSTUNREACH && error.ee.ee_origin == SO_EE_ORIGIN_ICMP6);
	CHECK(error.ee.ee_type == 3 && error.ee.ee_code == 0 && offender_is(&error, "fd77:1::1"));
	hops = 64;
	CHECK(setsockopt(six, IPPROTO_IPV6, IPV6_UNICAST_HOPS, &hops, sizeof(hops)) == 0);
	CHECK(sendto(six, "far6", 4, 0, (struct sockaddr *)&to6, sizeof(to6)) == 4);
	CHECK(queued_error(six, data, sizeof(data), SOL_IPV6, IPV6_RECVERR, &error) == 4);
	CHECK(error.ee.ee_errno == ECONNREFUSED && error.ee.ee_type == 1 && error.ee.ee_code == 4);
	CHECK(offender_is(&error, FAR6));
	close(six);
	if(!check_failures)
		printf("tracetest: sockets heard the routers' errors as on Linux\n");
}

// Runs `command` with sh(1), keeping what it writes, its standard error
// too, in `buffer`, and saying both: its exit code, or -1.
static int output(const char *command, char *buffer, size_t size) {
	char line[512];
	printf("tracetest: $ %s\n", command);
	snprintf(line, sizeof(line), "%s 2>&1", command);
	FILE *pipe = popen(line, "r");
	if(!pipe) {
		buffer[0] = 0;
		return -1;
	}
	size_t length = fread(buffer, 1, size - 1, pipe);
	buffer[length] = 0;
	int status = pclose(pipe);
	fputs(buffer, stdout);
	return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

// Whether a line of `text` starts with `start`, saying so if not.
static int has_line(const char *text, const char *start) {
	size_t length = strlen(start);
	for(const char *line = text; line && *line; line = strchr(line, '\n'), line = line ? line + 1 : NULL) {
		if(!strncmp(line, start, length))
			return 1;
	}
	printf("tracetest: no line starts \"%s\"\n", start);
	return 0;
}

// Whether `text` holds `needle`, saying so if not.
static int holds(const char *text, const char *needle) {
	if(strstr(text, needle))
		return 1;
	printf("tracetest: \"%s\" is not there\n", needle);
	return 0;
}

// Whether `text` holds a line for hop `hop` that starts with `start`,
// for what traceroute prints, and none for a hop past `last`.
static int hops_are(const char *text, const char *const *starts, int last) {
	int ok = 1;
	for(int i = 0; i < last; i++)
		ok &= has_line(text, starts[i]);
	char next[8];
	snprintf(next, sizeof(next), "%2d  ", last + 1);
	for(const char *line = text; line && *line; line = strchr(line, '\n'), line = line ? line + 1 : NULL) {
		if(!strncmp(line, next, strlen(next))) {
			printf("tracetest: a hop past %d\n", last);
			ok = 0;
		}
	}
	return ok;
}

static void traceroute(void) {
	static char text[8192];
	const char *numeric[] = {" 1  10.77.1.1  ", " 2  * * *", " 3  10.77.3.1  ", " 4  10.77.9.9  "};
	CHECK(output("traceroute -n " FAR, text, sizeof(text)) == 0);
	CHECK(has_line(text, "traceroute to 10.77.9.9 (10.77.9.9), 30 hops max, 60 byte packets"));
	CHECK(hops_are(text, numeric, 4));
	CHECK(holds(text, " ms"));
	// By ICMP echo requests.
	CHECK(output("traceroute -I -n -q 1 " FAR, text, sizeof(text)) == 0);
	const char *once[] = {" 1  10.77.1.1  ", " 2  *", " 3  10.77.3.1  ", " 4  10.77.9.9  "};
	CHECK(hops_are(text, once, 4));
	// By name, with the routers' names.
	CHECK(output("traceroute -q 1 far.trace.test", text, sizeof(text)) == 0);
	CHECK(has_line(text, "traceroute to far.trace.test (10.77.9.9), 30 hops max, 60 byte packets"));
	const char *named[] = {" 1  hop-1.trace.test (10.77.1.1)  ", " 2  *",
		" 3  hop-3.trace.test (10.77.3.1)  ", " 4  far.trace.test (10.77.9.9)  "};
	CHECK(hops_are(text, named, 4));
	// IPv6, as traceroute6 too.
	CHECK(output("traceroute6 -n -q 1 " FAR6, text, sizeof(text)) == 0);
	CHECK(has_line(text, "traceroute to fd77::99 (fd77::99), 30 hops max, 80 byte packets"));
	const char *six[] = {" 1  fd77:1::1  ", " 2  *", " 3  fd77:3::1  ", " 4  fd77::99  "};
	CHECK(hops_are(text, six, 4));
	CHECK(output("traceroute -6 -I -n -q 1 " FAR6, text, sizeof(text)) == 0);
	CHECK(hops_are(text, six, 4));
	// From the first hop on, and to a hop limit.
	CHECK(output("traceroute -n -q 1 -f 3 -m 3 " FAR, text, sizeof(text)) == 0);
	CHECK(has_line(text, " 3  10.77.3.1  ") && !strstr(text, "10.77.1.1"));
	if(!check_failures)
		printf("tracetest: traceroute found the hops by UDP and ICMP, IPv4 and IPv6, and their names\n");
}

static void mtr(void) {
	static char text[8192];
	CHECK(output("mtr --report -n -c 2 " FAR, text, sizeof(text)) == 0);
	CHECK(holds(text, "HOST: "));
	CHECK(holds(text, "  1.|-- 10.77.1.1 "));
	CHECK(holds(text, "  2.|-- ???"));
	CHECK(holds(text, "  3.|-- 10.77.3.1 "));
	CHECK(holds(text, "  4.|-- 10.77.9.9 "));
	CHECK(!strstr(text, "  5.|-- "));
	// With names and AS numbers, which it asks DNS for.
	CHECK(output("mtr --report -z -c 1 " FAR, text, sizeof(text)) == 0);
	CHECK(holds(text, "AS64500  hop-1.trace.test"));
	CHECK(holds(text, "AS64500  far.trace.test"));
	// IPv6, and its wide report.
	CHECK(output("mtr -6 --report-wide -n -c 1 " FAR6, text, sizeof(text)) == 0);
	CHECK(holds(text, "  1.|-- fd77:1::1 "));
	CHECK(holds(text, "  3.|-- fd77:3::1 "));
	CHECK(holds(text, "  4.|-- fd77::99 "));
	// Its UDP probes, which it sends through one socket without raw
	// sockets, said for the record alone.
	output("mtr -u --report -n -c 1 " FAR, text, sizeof(text));
	if(!check_failures)
		printf("tracetest: mtr measured each hop, by IPv4 and IPv6, with names and AS numbers\n");
}

static void ping(void) {
	static char text[4096];
	CHECK(output("ping -c 1 -t 1 " FAR, text, sizeof(text)) == 1);
	CHECK(holds(text, "From 10.77.1.1 icmp_seq=1 Time to live exceeded"));
	CHECK(output("ping -c 1 " FAR, text, sizeof(text)) == 0);
	CHECK(holds(text, "64 bytes from 10.77.9.9: icmp_seq=1 time="));
	CHECK(output("ping -6 -c 1 -t 3 " FAR6, text, sizeof(text)) == 1);
	CHECK(holds(text, "From fd77:3::1 icmp_seq=1 Time exceeded: Hop limit"));
	if(!check_failures)
		printf("tracetest: ping told the routers' errors as iputils does\n");
}

int main(void) {
	check_start("tracetest");
	RUN(make_tunnel);
	RUN(sockets);
	RUN(traceroute);
	RUN(mtr);
	RUN(ping);
	stopping = 1;
	return check_end();
}
