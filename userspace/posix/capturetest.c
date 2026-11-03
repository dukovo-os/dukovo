// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The network-capture boot test's program (ADR 0346): packet captures,
// as radix-network(2)'s open_capture gives them and as tcpdump(8) reads
// them through libpcap.
//
// It makes the tunnel cap0, 10.78.0.2/16 and fd78::2/64, and opens it,
// so that it sees what the machine sends there and can hand the machine
// packets that seem to come from 10.78.9.9. Then it checks the kernel's
// captures of lo, of cap0, of the network card net0 and of every
// interface at once: the frames, their headers and times, cut to a snap
// length or a read's buffer, picked by classic BPF filters, counted, and
// lost when the buffer is full; a capture whose interface is taken away;
// one refused without the control right; and the most there may be.
// Last it runs tcpdump on lo, any, cap0 and net0, with filters, into a
// file and back, by direction, and stopped by SIGINT.

#include <arpa/inet.h>
#include <fcntl.h>
#include <net/if.h>
#include <netinet/in.h>
#include <poll.h>
#include <pthread.h>
#include <radix/radix.h>
#include <stdint.h>
#include <sys/socket.h>

#include "check.h"

#define TUNNEL "cap0"
#define GONE "cap1"
#define PAYLOAD "capturetest!"
#define PAYLOAD_SIZE 12
// A UDP datagram of PAYLOAD on lo: Ethernet, IPv4 and UDP headers, and
// its record in a read: the header and the frame, to a multiple of 8.
#define LO_FRAME (14 + 20 + 8 + PAYLOAD_SIZE)
#define LO_RECORD 80
#define HEADER ((size_t)sizeof(struct radix_capture_header))

// A classic BPF instruction, as Linux's struct sock_filter.
struct insn {
	uint16_t code;
	uint8_t jt;
	uint8_t jf;
	uint32_t k;
};

// The tunnel's file; the port the router thread sends cap0 datagrams
// to, every 100 ms, while it is not 0; and whether it is to stop.
static int tunnel = -1;
static volatile uint16_t inject_port;
static volatile int stopping;
static uint32_t lo, cap0, net0;

// What reads of captures take.
static uint8_t frames[1 << 17];

static int status_errno(uint64_t status) {
	switch(RADIX_STATUS_DOMAIN(status)) {
	case RADIX_KERNEL_ERROR_DOMAIN:
		return radix_kernel_errno(status);
	case RADIX_FILE_ERROR_DOMAIN:
		return radix_file_errno(status);
	default:
		return EIO;
	}
}

// Opens a capture of interface `index`, or of every one for 0, through
// network capability `net`, into the slot of the lowest free descriptor:
// the descriptor, or -1 with errno set.
static int capture_on(uint32_t net, uint32_t index, uint32_t snap, uint32_t buffer,
		uint32_t flags) {
	int fd = open("/devices/null", O_RDONLY);
	if(fd < 0)
		return -1;
	close(fd);
	uint64_t status = radix_network_open_capture(net, RADIX_STARTUP_SLOT_FD_BASE + (uint32_t)fd,
			index, snap, buffer, flags);
	if(status) {
		errno = status_errno(status);
		return -1;
	}
	return fd;
}

// A non-blocking capture, through the network capability the test was
// given.
static int capture(uint32_t index, uint32_t snap, uint32_t buffer) {
	return capture_on(RADIX_STARTUP_SLOT_NETWORK, index, snap, buffer,
			RADIX_PIPE_FLAGS_NON_BLOCKING | RADIX_PIPE_FLAGS_CLOSE_ON_EXEC);
}

// radix-file(2)'s control of capture `fd`: 0, or -1 with errno set.
static int control(int fd, uint32_t request, const void *buffer, uint64_t size) {
	struct radix_file_control_results results;
	uint64_t status = radix_file_control(RADIX_STARTUP_SLOT_FD_BASE + (uint32_t)fd, request, 0,
			(uint64_t)(uintptr_t)buffer, size, &results);
	if(status) {
		errno = status_errno(status);
		return -1;
	}
	return 0;
}

static int set_filter(int fd, const struct insn *program, size_t count) {
	return control(fd, RADIX_NETWORK_CAPTURE_FILTER, program, count * sizeof(*program));
}

static struct radix_capture_counts counts_of(int fd) {
	struct radix_capture_counts counts = {0, 0};
	CHECK(control(fd, RADIX_NETWORK_CAPTURE_COUNTS, &counts, sizeof(counts)) == 0);
	return counts;
}

// Whether `fd`'s capture has kept `count` frames within 5 seconds.
static int kept(int fd, uint64_t count) {
	for(int i = 0; i < 500; i++) {
		if(counts_of(fd).received >= count)
			return 1;
		check_sleep_ms(10);
	}
	printf("capturetest: the capture kept %llu frames, not %llu\n",
			(unsigned long long)counts_of(fd).received, (unsigned long long)count);
	return 0;
}

// Sends `size` bytes of `data` to `address`, IPv4 or IPv6, port `port`,
// from a new socket: whether it did.
static int send_to(const char *address, uint16_t port, const void *data, size_t size) {
	struct sockaddr_storage to = {0};
	socklen_t length;
	if(strchr(address, ':')) {
		struct sockaddr_in6 *six = (struct sockaddr_in6 *)&to;
		six->sin6_family = AF_INET6;
		six->sin6_port = htons(port);
		inet_pton(AF_INET6, address, &six->sin6_addr);
		length = sizeof(*six);
	} else {
		struct sockaddr_in *four = (struct sockaddr_in *)&to;
		four->sin_family = AF_INET;
		four->sin_port = htons(port);
		inet_pton(AF_INET, address, &four->sin_addr);
		length = sizeof(*four);
	}
	int fd = socket(to.ss_family, SOCK_DGRAM, 0);
	if(fd < 0)
		return 0;
	ssize_t sent = sendto(fd, data, size, 0, (struct sockaddr *)&to, length);
	close(fd);
	return sent == (ssize_t)size;
}

static int send_payload(const char *address, uint16_t port) {
	return send_to(address, port, PAYLOAD, PAYLOAD_SIZE);
}

// A UDP socket bound to `address` port `port`, so that what is sent
// there is taken and not answered with "port unreachable".
static int bound(const char *address, uint16_t port) {
	struct sockaddr_storage at = {0};
	socklen_t length;
	if(strchr(address, ':')) {
		struct sockaddr_in6 *six = (struct sockaddr_in6 *)&at;
		six->sin6_family = AF_INET6;
		six->sin6_port = htons(port);
		inet_pton(AF_INET6, address, &six->sin6_addr);
		length = sizeof(*six);
	} else {
		struct sockaddr_in *four = (struct sockaddr_in *)&at;
		four->sin_family = AF_INET;
		four->sin_port = htons(port);
		inet_pton(AF_INET, address, &four->sin_addr);
		length = sizeof(*four);
	}
	int fd = socket(at.ss_family, SOCK_DGRAM, 0);
	CHECK(fd >= 0);
	CHECK(bind(fd, (struct sockaddr *)&at, length) == 0);
	return fd;
}

static uint16_t get16(const uint8_t *p) {
	return (uint16_t)(p[0] << 8 | p[1]);
}

static void put16(uint8_t *p, uint16_t value) {
	p[0] = value >> 8;
	p[1] = value & 0xff;
}

// The header of the record at `at`.
static struct radix_capture_header header_at(const uint8_t *at) {
	struct radix_capture_header h;
	memcpy(&h, at, sizeof(h));
	return h;
}

// The bytes a record of `captured` bytes takes.
static size_t record(size_t captured) {
	return (HEADER + captured + 7) & ~(size_t)7;
}

// Whether `time`, a header's, is now: by the time of day, or by the time
// since the machine started while that is not known.
static int is_now(uint64_t time) {
	struct timespec real, since;
	clock_gettime(CLOCK_REALTIME, &real);
	clock_gettime(CLOCK_MONOTONIC, &since);
	long long t = (long long)time;
	long long r = (long long)real.tv_sec * 1000000000LL + real.tv_nsec;
	long long s = (long long)since.tv_sec * 1000000000LL + since.tv_nsec;
	return llabs(t - r) < 10000000000LL || llabs(t - s) < 10000000000LL;
}

// Whether the frame at `frame`, of `h`, is the IPv4 datagram of PAYLOAD
// lo carried to `port`, as Linux's lo gives it: an Ethernet header of
// zero addresses before it.
static int lo_datagram(const struct radix_capture_header *h, const uint8_t *frame,
		uint16_t port) {
	static const uint8_t zeros[12];
	return h->length == LO_FRAME && h->captured == LO_FRAME && h->interface == lo
			&& h->link == RADIX_NETWORK_CAPTURE_LINK_ETHERNET && h->outgoing == 0
			&& h->reserved == 0 && !memcmp(frame, zeros, 12) && get16(frame + 12) == 0x0800
			&& frame[14] == 0x45 && frame[23] == 17 && get16(frame + 36) == port
			&& !memcmp(frame + 42, PAYLOAD, PAYLOAD_SIZE);
}

// A filter for UDP datagrams to `port` in Ethernet frames, IPv4 or IPv6,
// as tcpdump's "udp dst port PORT" without fragments, IP options or
// IPv6 extension headers, keeping `cut` bytes of each.
#define UDP_FILTER 13
static void udp_filter(struct insn program[UDP_FILTER], uint16_t port, uint32_t cut) {
	const struct insn filter[UDP_FILTER] = {
		{0x28, 0, 0, 12}, // ldh [12]
		{0x15, 0, 4, 0x0800}, // jeq #0x800, else to 6
		{0x30, 0, 0, 23}, // ldb [23]
		{0x15, 0, 8, 17}, // jeq #17, else drop
		{0x28, 0, 0, 36}, // ldh [36]
		{0x15, 5, 6, port}, // jeq #port: keep, else drop
		{0x15, 0, 5, 0x86dd}, // jeq #0x86dd, else drop
		{0x30, 0, 0, 20}, // ldb [20]
		{0x15, 0, 3, 17}, // jeq #17, else drop
		{0x28, 0, 0, 56}, // ldh [56]
		{0x15, 0, 1, port}, // jeq #port, else drop
		{0x06, 0, 0, cut}, // ret #cut
		{0x06, 0, 0, 0}, // ret #0
	};
	memcpy(program, filter, sizeof(filter));
}

// A capture of lo that keeps what is sent to UDP port `port` alone, as
// other programs may use lo too.
static int lo_port(uint16_t port, uint32_t snap, uint32_t buffer, uint32_t flags) {
	int fd = capture_on(RADIX_STARTUP_SLOT_NETWORK, lo, snap, buffer, flags);
	struct insn program[UDP_FILTER];
	udp_filter(program, port, RADIX_NETWORK_CAPTURE_SNAP_MAX);
	CHECK(fd >= 0 && set_filter(fd, program, UDP_FILTER) == 0);
	return fd;
}

typedef int (*matcher)(const struct radix_capture_header *, const uint8_t *, uint16_t);

// How many of the records in the first `got` bytes of `frames` `match`
// picks, with `port`.
static int matching(ssize_t got, matcher match, uint16_t port) {
	int count = 0;
	for(ssize_t at = 0; at + (ssize_t)HEADER <= got;) {
		struct radix_capture_header h = header_at(frames + at);
		count += match(&h, frames + at + HEADER, port);
		at += record(h.captured);
	}
	return count;
}

// Whether a record is a datagram of PAYLOAD sent out a tunnel, cap0 or
// cap1, to `port` by IPv4.
static int sent_out(const struct radix_capture_header *h, const uint8_t *frame, uint16_t port) {
	return h->length == 40 && h->captured == 40 && h->link == RADIX_NETWORK_CAPTURE_LINK_RAW
			&& h->outgoing == 1 && frame[0] == 0x45 && frame[9] == 17
			&& get16(frame + 22) == port && !memcmp(frame + 28, PAYLOAD, PAYLOAD_SIZE);
}

// The same by IPv6.
static int sent_out6(const struct radix_capture_header *h, const uint8_t *frame, uint16_t port) {
	return h->length == 60 && h->captured == 60 && h->link == RADIX_NETWORK_CAPTURE_LINK_RAW
			&& h->outgoing == 1 && frame[0] >> 4 == 6 && frame[6] == 17
			&& get16(frame + 42) == port;
}

// Whether a record is the router's datagram to `port`, which cap0
// received.
static int received_in(const struct radix_capture_header *h, const uint8_t *frame,
		uint16_t port) {
	return h->length == 40 && h->link == RADIX_NETWORK_CAPTURE_LINK_RAW && h->outgoing == 0
			&& h->interface == cap0 && frame[9] == 17 && get16(frame + 22) == port
			&& !memcmp(frame + 12, "\x0a\x4e\x09\x09", 4);
}

static int of_lo(const struct radix_capture_header *h, const uint8_t *frame, uint16_t port) {
	return lo_datagram(h, frame, port);
}

static int of_cap0(const struct radix_capture_header *h, const uint8_t *frame, uint16_t port) {
	return h->interface == cap0 && sent_out(h, frame, port);
}

// The tunnel's router: it takes what the machine sends cap0, and while
// `inject_port` is set sends the machine a UDP datagram of PAYLOAD from
// 10.78.9.9 to that port every 100 ms.
static void *router(void *unused) {
	(void)unused;
	uint8_t packet[2048];
	long long next = 0;
	while(!stopping) {
		uint16_t port = inject_port;
		if(port && check_now_ns() >= next) {
			uint8_t out[20 + 8 + PAYLOAD_SIZE] = {0};
			out[0] = 0x45;
			put16(out + 2, sizeof(out));
			out[8] = 64;
			out[9] = 17;
			inet_pton(AF_INET, "10.78.9.9", out + 12);
			inet_pton(AF_INET, "10.78.0.2", out + 16);
			uint32_t sum = 0;
			for(int i = 0; i < 20; i += 2)
				sum += get16(out + i);
			while(sum >> 16)
				sum = (sum & 0xffff) + (sum >> 16);
			put16(out + 10, (uint16_t)~sum);
			put16(out + 20, port);
			put16(out + 22, port);
			put16(out + 24, 8 + PAYLOAD_SIZE);
			memcpy(out + 28, PAYLOAD, PAYLOAD_SIZE);
			(void)!write(tunnel, out, sizeof(out));
			next = check_now_ns() + 100000000LL;
		}
		struct pollfd p = {tunnel, POLLIN, 0};
		if(poll(&p, 1, 20) > 0)
			(void)!read(tunnel, packet, sizeof(packet));
	}
	return NULL;
}

// Makes tunnel `name`, at 10.`net`.0.2/16 and fd`net`::2/64, and opens
// it: its file, with its number in `*index`.
static int make_tunnel(const char *name, int net, uint32_t *index) {
	struct radix_network_add_link_results made;
	CHECK(radix_network_add_link(RADIX_STARTUP_SLOT_NETWORK, RADIX_LINK_KIND_TUNNEL, 0, 0, name,
				  strlen(name), &made)
			== 0);
	*index = made.interface;
	int fd = open("/devices/null", O_RDONLY);
	CHECK(fd >= 0);
	close(fd);
	CHECK(radix_network_open_tunnel(RADIX_STARTUP_SLOT_NETWORK, RADIX_STARTUP_SLOT_FD_BASE + fd,
				  made.interface, RADIX_PIPE_FLAGS_NON_BLOCKING | RADIX_PIPE_FLAGS_CLOSE_ON_EXEC)
			== 0);
	struct radix_interface_address four = {.ip = {10, (uint8_t)net, 0, 2}, .prefix = 16};
	CHECK(radix_network_configure(RADIX_STARTUP_SLOT_NETWORK, made.interface, &four, sizeof(four),
				  NULL, 0)
			== 0);
	struct radix_ipv6_settings settings = {.flags = RADIX_IPV6_FLAGS_ENABLED};
	struct radix_interface_address_ipv6 six = {.prefix = 64};
	char text[32];
	snprintf(text, sizeof(text), "fd%d::2", net);
	inet_pton(AF_INET6, text, six.ip);
	CHECK(radix_network_configure_ipv6(RADIX_STARTUP_SLOT_NETWORK, made.interface, &settings,
				  sizeof(settings), &six, sizeof(six), NULL, 0)
			== 0);
	// Its IPv6 address may wait for a moment.
	for(int i = 0; i < 50; i++) {
		int probe = socket(AF_INET6, SOCK_DGRAM, 0);
		struct sockaddr_in6 at = {.sin6_family = AF_INET6};
		inet_pton(AF_INET6, text, &at.sin6_addr);
		int failed = bind(probe, (struct sockaddr *)&at, sizeof(at));
		close(probe);
		if(!failed)
			break;
		check_sleep_ms(100);
	}
	return fd;
}

static void setup(void) {
	lo = if_nametoindex("lo");
	net0 = if_nametoindex("net0");
	CHECK(lo == 1);
	CHECK(net0 == 2);
	tunnel = make_tunnel(TUNNEL, 78, &cap0);
	CHECK(if_nametoindex(TUNNEL) == cap0);
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, router, NULL) == 0);
	CHECK(pthread_detach(thread) == 0);
	printf("capturetest: " TUNNEL " is up, with its router\n");
}

// A capture's frames, headers and times; reads that take several, and
// those cut to their buffer.
static void frames_of_lo(void) {
	int four = bound("127.0.0.1", 7801);
	int six = bound("::1", 7801);
	int fd = lo_port(7801, RADIX_NETWORK_CAPTURE_SNAP_MAX, 0,
			RADIX_PIPE_FLAGS_NON_BLOCKING | RADIX_PIPE_FLAGS_CLOSE_ON_EXEC);
	CHECK_ERROR(read(fd, frames, sizeof(frames)), EAGAIN);
	struct pollfd p = {fd, POLLIN, 0};
	CHECK(poll(&p, 1, 0) == 0);

	CHECK(send_payload("127.0.0.1", 7801));
	CHECK(poll(&p, 1, 5000) == 1 && p.revents == POLLIN);
	ssize_t got = read(fd, frames, sizeof(frames));
	CHECK(got == LO_RECORD);
	struct radix_capture_header h = header_at(frames);
	CHECK(lo_datagram(&h, frames + HEADER, 7801));
	CHECK(is_now(h.time));
	CHECK(!memcmp(frames + HEADER + LO_FRAME, "\0\0", LO_RECORD - HEADER - LO_FRAME));
	CHECK(!memcmp(frames + HEADER + 26, "\x7f\0\0\x01", 4));

	// IPv6 on lo, under the IPv6 type.
	CHECK(send_payload("::1", 7801));
	CHECK(kept(fd, 2));
	got = read(fd, frames, sizeof(frames));
	h = header_at(frames);
	CHECK(got == (ssize_t)record(14 + 40 + 8 + PAYLOAD_SIZE));
	CHECK(h.length == 14 + 40 + 8 + PAYLOAD_SIZE && h.captured == h.length);
	CHECK(get16(frames + HEADER + 12) == 0x86dd && frames[HEADER + 14] >> 4 == 6);
	CHECK(frames[HEADER + 20] == 17 && get16(frames + HEADER + 56) == 7801);

	// Three, read at once.
	for(int i = 0; i < 3; i++)
		CHECK(send_payload("127.0.0.1", 7801));
	CHECK(kept(fd, 5));
	got = read(fd, frames, sizeof(frames));
	CHECK(got == 3 * LO_RECORD);
	for(int i = 0; i < 3; i++) {
		h = header_at(frames + i * LO_RECORD);
		CHECK(lo_datagram(&h, frames + i * LO_RECORD + HEADER, 7801));
	}

	// One cut to a buffer of its header and 20 bytes: the rest goes.
	CHECK(send_payload("127.0.0.1", 7801));
	CHECK(kept(fd, 6));
	CHECK(read(fd, frames, HEADER + 20) == (ssize_t)(HEADER + 20));
	h = header_at(frames);
	CHECK(h.length == LO_FRAME && h.captured == 20);
	CHECK_ERROR(read(fd, frames, sizeof(frames)), EAGAIN);
	CHECK_ERROR(read(fd, frames, HEADER - 8), EINVAL);

	// Two, and room for one and a half: one, then the other.
	CHECK(send_payload("127.0.0.1", 7801));
	CHECK(send_payload("127.0.0.1", 7801));
	CHECK(kept(fd, 8));
	CHECK(read(fd, frames, LO_RECORD + LO_RECORD / 2) == LO_RECORD);
	CHECK(read(fd, frames, LO_RECORD + LO_RECORD / 2) == LO_RECORD);
	h = header_at(frames);
	CHECK(lo_datagram(&h, frames + HEADER, 7801));

	struct radix_capture_counts counts = counts_of(fd);
	CHECK(counts.received == 8 && counts.dropped == 0);
	CHECK_ERROR(write(fd, "x", 1), EOPNOTSUPP);
	CHECK(close(fd) == 0);

	// A blocking capture's read waits for a frame.
	fd = lo_port(7801, 2048, 0, RADIX_PIPE_FLAGS_CLOSE_ON_EXEC);
	pid_t pid = check_fork();
	if(pid == 0) {
		check_sleep_ms(300);
		CHECK(send_payload("127.0.0.1", 7801));
		check_exit_child();
	}
	long long start = check_now_ns();
	CHECK(read(fd, frames, sizeof(frames)) == LO_RECORD);
	CHECK(check_now_ns() - start > 200000000LL);
	CHECK_CHILD(pid);
	close(fd);
	close(four);
	close(six);
	printf("capturetest: lo's frames came whole, timed, and cut to their reads\n");
}

// Snap lengths, buffers and interfaces a capture may and may not have.
static void limits(void) {
	int four = bound("127.0.0.1", 7801);
	int fd = lo_port(7801, 20, 0, RADIX_PIPE_FLAGS_NON_BLOCKING);
	CHECK(send_payload("127.0.0.1", 7801));
	CHECK(kept(fd, 1));
	CHECK(read(fd, frames, sizeof(frames)) == (ssize_t)record(20));
	struct radix_capture_header h = header_at(frames);
	CHECK(h.length == LO_FRAME && h.captured == 20);
	close(fd);
	close(four);

	CHECK_ERROR(capture(lo, 0, 0), EINVAL);
	CHECK_ERROR(capture(lo, RADIX_NETWORK_CAPTURE_SNAP_MAX + 1, 0), EINVAL);
	CHECK_ERROR(capture(lo, 100, RADIX_NETWORK_CAPTURE_BUFFER_MIN - 8), EINVAL);
	CHECK_ERROR(capture(lo, 100, RADIX_NETWORK_CAPTURE_BUFFER_MAX + 8), EINVAL);
	CHECK_ERROR(capture(200, 100, 0), ENXIO);
	fd = capture(lo, 100, RADIX_NETWORK_CAPTURE_BUFFER_MIN);
	CHECK(fd >= 0);
	close(fd);
	fd = capture(lo, 100, RADIX_NETWORK_CAPTURE_BUFFER_MAX);
	CHECK(fd >= 0);
	close(fd);
	printf("capturetest: captures kept their snap lengths and refused what is out of range\n");
}

// Filters: what they pick and cut, what setting one throws away, and the
// programs the kernel refuses.
static void filters(void) {
	int to2 = bound("127.0.0.1", 7802);
	int to3 = bound("127.0.0.1", 7803);
	int fd = capture(lo, RADIX_NETWORK_CAPTURE_SNAP_MAX, 0);
	CHECK(fd >= 0);
	struct insn program[UDP_FILTER];
	udp_filter(program, 7802, 50);
	CHECK(set_filter(fd, program, UDP_FILTER) == 0);
	CHECK(send_payload("127.0.0.1", 7803));
	CHECK(send_payload("127.0.0.1", 7802));
	CHECK(kept(fd, 1));
	check_sleep_ms(200);
	struct radix_capture_counts counts = counts_of(fd);
	CHECK(counts.received == 1 && counts.dropped == 0);
	CHECK(read(fd, frames, sizeof(frames)) == (ssize_t)record(50));
	struct radix_capture_header h = header_at(frames);
	CHECK(h.length == LO_FRAME && h.captured == 50 && get16(frames + HEADER + 36) == 7802);

	// Setting it again throws away what waits, and counts again.
	CHECK(send_payload("127.0.0.1", 7802));
	CHECK(kept(fd, 2));
	CHECK(set_filter(fd, program, UDP_FILTER) == 0);
	counts = counts_of(fd);
	CHECK(counts.received == 0 && counts.dropped == 0);
	CHECK_ERROR(read(fd, frames, sizeof(frames)), EAGAIN);

	// A load past the frame's end drops it, as on Linux.
	const struct insn past[2] = {{0x20, 0, 0, 1000}, {0x06, 0, 0, 0xffffffff}};
	CHECK(set_filter(fd, past, 2) == 0);
	CHECK(send_payload("127.0.0.1", 7802));
	check_sleep_ms(300);
	CHECK(counts_of(fd).received == 0);

	// Without one, every frame is kept.
	CHECK(control(fd, RADIX_NETWORK_CAPTURE_FILTER, NULL, 0) == 0);
	CHECK(send_payload("127.0.0.1", 7803));
	CHECK(send_payload("127.0.0.1", 7802));
	CHECK(kept(fd, 2));
	check_sleep_ms(200);
	ssize_t got = read(fd, frames, sizeof(frames));
	CHECK(matching(got, of_lo, 7803) == 1 && matching(got, of_lo, 7802) == 1);

	// Programs the kernel does not run.
	const struct insn opcode[2] = {{0xff, 0, 0, 0}, {0x06, 0, 0, 0}};
	const struct insn jump[2] = {{0x15, 5, 0, 1}, {0x06, 0, 0, 0}};
	const struct insn last[1] = {{0x28, 0, 0, 12}};
	const struct insn memory[2] = {{0x02, 0, 0, 16}, {0x06, 0, 0, 0}};
	const struct insn divide[2] = {{0x34, 0, 0, 0}, {0x06, 0, 0, 0}};
	CHECK_ERROR(set_filter(fd, opcode, 2), EINVAL);
	CHECK_ERROR(set_filter(fd, jump, 2), EINVAL);
	CHECK_ERROR(set_filter(fd, last, 1), EINVAL);
	CHECK_ERROR(set_filter(fd, memory, 2), EINVAL);
	CHECK_ERROR(set_filter(fd, divide, 2), EINVAL);
	CHECK_ERROR(control(fd, RADIX_NETWORK_CAPTURE_FILTER, program, 12), EINVAL);
	CHECK_ERROR(control(fd, RADIX_NETWORK_CAPTURE_FILTER, (void *)8, 16), EFAULT);
	static struct insn many[4097];
	for(int i = 0; i < 4097; i++)
		many[i] = (struct insn){0x06, 0, 0, 0};
	CHECK_ERROR(set_filter(fd, many, 4097), EINVAL);
	CHECK(set_filter(fd, many, 4096) == 0);
	struct radix_capture_counts small;
	CHECK_ERROR(control(fd, RADIX_NETWORK_CAPTURE_COUNTS, &small, 8), EINVAL);
	CHECK_ERROR(control(fd, 0x52430099, &small, sizeof(small)), ENOTTY);
	close(fd);
	close(to2);
	close(to3);
	printf("capturetest: filters picked and cut frames, and bad programs were refused\n");
}

// A full buffer: the frames that find no room are lost, and counted.
static void drops(void) {
	int to = bound("127.0.0.1", 7804);
	int fd = capture(lo, RADIX_NETWORK_CAPTURE_SNAP_MAX, RADIX_NETWORK_CAPTURE_BUFFER_MIN);
	CHECK(fd >= 0);
	struct insn program[UDP_FILTER];
	udp_filter(program, 7804, RADIX_NETWORK_CAPTURE_SNAP_MAX);
	CHECK(set_filter(fd, program, UDP_FILTER) == 0);
	static char big[1000];
	memset(big, 'd', sizeof(big));
	for(int i = 0; i < 100; i++)
		CHECK(send_to("127.0.0.1", 7804, big, sizeof(big)));
	// Until the count stops.
	uint64_t received = 0;
	for(int i = 0; i < 100; i++) {
		check_sleep_ms(50);
		uint64_t now = counts_of(fd).received;
		if(now == received && now > 0)
			break;
		received = now;
	}
	size_t size = record(14 + 20 + 8 + sizeof(big));
	size_t fit = RADIX_NETWORK_CAPTURE_BUFFER_MIN / size;
	struct radix_capture_counts counts = counts_of(fd);
	printf("capturetest: %llu frames, %llu lost\n", (unsigned long long)counts.received,
			(unsigned long long)counts.dropped);
	CHECK(counts.received > fit && counts.dropped == counts.received - fit);
	CHECK(read(fd, frames, sizeof(frames)) == (ssize_t)(fit * size));
	CHECK(counts_of(fd).dropped == counts.dropped);
	close(fd);

	// A buffer of a piece and a half, which the kernel keeps in pieces of
	// 64 KiB: frames that go round its end, and across a piece's, come
	// whole.
	fd = capture(lo, RADIX_NETWORK_CAPTURE_SNAP_MAX, RADIX_NETWORK_CAPTURE_BUFFER_MIN * 3 / 2);
	CHECK(fd >= 0);
	CHECK(set_filter(fd, program, UDP_FILTER) == 0);
	int whole = 1;
	for(int round = 0; round < 20; round++) {
		for(int i = 0; i < 10; i++) {
			memset(big, 'a' + (round * 10 + i) % 26, sizeof(big));
			CHECK(send_to("127.0.0.1", 7804, big, sizeof(big)));
		}
		CHECK(kept(fd, (uint64_t)(round + 1) * 10));
		ssize_t got = read(fd, frames, sizeof(frames));
		CHECK(got == (ssize_t)(10 * size));
		for(int i = 0; i < 10 && got == (ssize_t)(10 * size); i++) {
			const uint8_t *at = frames + i * size;
			struct radix_capture_header h = header_at(at);
			whole &= h.captured == 14 + 20 + 8 + sizeof(big) && get16(at + HEADER + 36) == 7804;
			for(size_t b = 0; b < sizeof(big); b++)
				whole &= at[HEADER + 42 + b] == 'a' + (round * 10 + i) % 26;
		}
	}
	CHECK(whole);
	CHECK(counts_of(fd).dropped == 0);
	close(fd);
	close(to);
	printf("capturetest: a full buffer lost frames, and counted them\n");
}

// Captures of cap0, IP packets alone, both ways, and of every interface.
static void tunnels(void) {
	int fd = capture(cap0, RADIX_NETWORK_CAPTURE_SNAP_MAX, 0);
	CHECK(fd >= 0);
	CHECK(send_payload("10.78.9.9", 7805));
	CHECK(send_payload("fd78::99", 7805));
	CHECK(kept(fd, 2));
	check_sleep_ms(200);
	ssize_t got = read(fd, frames, sizeof(frames));
	CHECK(matching(got, of_cap0, 7805) == 1);
	CHECK(matching(got, sent_out6, 7805) == 1);

	// What the router sends the machine, received.
	int to = bound("10.78.0.2", 7806);
	inject_port = 7806;
	check_sleep_ms(350);
	inject_port = 0;
	check_sleep_ms(200);
	got = read(fd, frames, sizeof(frames));
	CHECK(matching(got, received_in, 7806) >= 1);
	close(to);
	close(fd);

	// Every interface: lo's and cap0's frames, each by its interface.
	int four = bound("127.0.0.1", 7801);
	fd = capture(0, RADIX_NETWORK_CAPTURE_SNAP_MAX, 0);
	CHECK(fd >= 0);
	CHECK(send_payload("127.0.0.1", 7801));
	CHECK(send_payload("10.78.9.9", 7805));
	CHECK(kept(fd, 2));
	check_sleep_ms(200);
	got = read(fd, frames, sizeof(frames));
	CHECK(matching(got, of_lo, 7801) == 1 && matching(got, of_cap0, 7805) == 1);
	close(fd);
	close(four);
	printf("capturetest: " TUNNEL "'s packets were captured both ways, and every interface's at "
		   "once\n");
}

// A capture whose interface is taken away: what waits, then ENXIO.
static void gone(void) {
	uint32_t index;
	int file = make_tunnel(GONE, 79, &index);
	int fd = capture(index, RADIX_NETWORK_CAPTURE_SNAP_MAX, 0);
	CHECK(fd >= 0);
	CHECK(send_payload("10.79.9.9", 7807));
	CHECK(kept(fd, 1));
	CHECK(radix_network_remove_link(RADIX_STARTUP_SLOT_NETWORK, index) == 0);
	struct pollfd p = {fd, POLLIN, 0};
	CHECK(poll(&p, 1, 1000) == 1 && p.revents == (POLLIN | POLLERR));
	ssize_t got = read(fd, frames, sizeof(frames));
	CHECK(matching(got, sent_out, 7807) == 1);
	CHECK_ERROR(read(fd, frames, sizeof(frames)), ENXIO);
	CHECK(poll(&p, 1, 0) == 1 && p.revents == (POLLIN | POLLERR));
	CHECK_ERROR(capture(index, 100, 0), ENXIO);
	close(fd);
	close(file);
	printf("capturetest: a capture of a tunnel taken away read what waited, then ENXIO\n");
}

// Runs `command` with sh(1), keeping what it writes, its standard error
// too, in `buffer`, and saying both: its exit code, or -1.
static int output(const char *command, char *buffer, size_t size) {
	char line[512];
	printf("capturetest: $ %s\n", command);
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

// Whether `text` holds `needle`, saying so if not.
static int holds(const char *text, const char *needle) {
	if(strstr(text, needle))
		return 1;
	printf("capturetest: \"%s\" is not there\n", needle);
	return 0;
}

// How many times `text` holds `needle`.
static int times(const char *text, const char *needle) {
	int count = 0;
	for(const char *at = strstr(text, needle); at; at = strstr(at + 1, needle))
		count++;
	return count;
}

// Deletes the network capabilities but the one in the last slot, which
// it first derives from the first with every right it has but control.
static void without_control(void) {
	struct radix_capability_info_results info;
	CHECK(radix_capability_info(RADIX_STARTUP_SLOT_NETWORK, &info) == 0);
	uint32_t slot = RADIX_STARTUP_SLOT_NETWORK + RADIX_STARTUP_NETWORK_SLOTS - 1;
	radix_capability_delete(slot);
	CHECK(radix_network_restrict(RADIX_STARTUP_SLOT_NETWORK, slot, 0, 65536,
				  info.rights & ~(uint64_t)RADIX_RIGHTS_CONTROL)
			== 0);
	for(uint32_t i = 0; i < RADIX_STARTUP_NETWORK_SLOTS - 1; i++)
		radix_capability_delete(RADIX_STARTUP_SLOT_NETWORK + i);
}

// Captures need the control right; tcpdump says how to get it.
static void rights(void) {
	pid_t pid = check_fork();
	if(pid == 0) {
		without_control();
		uint32_t slot = RADIX_STARTUP_SLOT_NETWORK + RADIX_STARTUP_NETWORK_SLOTS - 1;
		CHECK_ERROR(capture_on(slot, lo, 100, 0, 0), EACCES);
		CHECK_ERROR(capture_on(RADIX_STARTUP_SLOT_NETWORK, lo, 100, 0, 0), EBADF);
		static char said[4096];
		CHECK(output("tcpdump -i lo -c 1", said, sizeof(said)) == 1);
		CHECK(holds(said, "tcpdump: lo: You don't have permission to perform this capture on "
						  "that device\n(capturing needs admin.network: run it with elevate(8))\n"));
		check_exit_child();
	}
	CHECK_CHILD(pid);
	printf("capturetest: a capture needed the control right, and tcpdump said so\n");
}

// The most captures there may be: one more is refused, until one closes.
static void most(void) {
	int fds[RADIX_NETWORK_CAPTURES_MAX];
	for(unsigned i = 0; i < RADIX_NETWORK_CAPTURES_MAX; i++) {
		fds[i] = capture(lo, 100, 0);
		CHECK(fds[i] >= 0);
	}
	CHECK_ERROR(capture(lo, 100, 0), EBUSY);
	static char said[4096];
	CHECK(output("tcpdump -i lo -c 1", said, sizeof(said)) == 1);
	CHECK(holds(said, "tcpdump: lo: 16 packet captures are open, the most there may be\n"));
	close(fds[0]);
	fds[0] = capture(lo, 100, 0);
	CHECK(fds[0] >= 0);
	for(unsigned i = 0; i < RADIX_NETWORK_CAPTURES_MAX; i++)
		close(fds[i]);
	printf("capturetest: %u captures were the most there may be\n",
			(unsigned)RADIX_NETWORK_CAPTURES_MAX);
}

// A child that sends `address` port `port` a datagram of PAYLOAD every
// 100 ms until it is killed.
static pid_t sending(const char *address, uint16_t port) {
	pid_t pid = check_fork();
	if(pid == 0) {
		for(int i = 0; i < 600; i++) {
			send_payload(address, port);
			check_sleep_ms(100);
		}
		_exit(0);
	}
	return pid;
}

static void stop(pid_t pid) {
	kill(pid, SIGKILL);
	check_wait(pid);
}

// tcpdump `arguments` while `address` port `port` is sent datagrams:
// its exit code, and what it wrote in `said`.
static int tcpdump_while(const char *address, uint16_t port, const char *arguments, char *said,
		size_t size) {
	char command[256];
	snprintf(command, sizeof(command), "timeout 30 tcpdump %s", arguments);
	pid_t sender = sending(address, port);
	int code = output(command, said, size);
	stop(sender);
	return code;
}

static void tcpdump_lists(void) {
	static char said[8192];
	CHECK(output("tcpdump --version", said, sizeof(said)) == 0);
	CHECK(holds(said, "tcpdump version 4.99.6\n"));
	CHECK(holds(said, "libpcap version 1.10.7"));
	CHECK(output("tcpdump -D", said, sizeof(said)) == 0);
	CHECK(holds(said, ".lo [Up, Running, Loopback]\n"));
	CHECK(holds(said, ".net0 [Up, Running, Connected]\n"));
	CHECK(holds(said, "." TUNNEL " [Up"));
	CHECK(holds(said, ".any (Pseudo-device that captures on all interfaces) [Up, Running]\n"));
	CHECK(output("tcpdump -i nosuch0 -c 1", said, sizeof(said)) == 1);
	CHECK(holds(said, "tcpdump: nosuch0: No such device exists\n(if_nametoindex: No such device)\n"));
	printf("capturetest: tcpdump listed the interfaces\n");
}

static void tcpdump_lo(void) {
	static char said[16384];
	CHECK(tcpdump_while("127.0.0.1", 7810, "-i lo -n -e -c 3 udp dst port 7810", said,
				  sizeof(said))
			== 0);
	CHECK(holds(said, "listening on lo, link-type EN10MB (Ethernet), snapshot length 262144 bytes\n"));
	CHECK(holds(said, " 00:00:00:00:00:00 > 00:00:00:00:00:00, ethertype IPv4 (0x0800), length 54: "
					  "127.0.0.1."));
	CHECK(times(said, " > 127.0.0.1.7810: UDP, length 12\n") == 3);
	CHECK(holds(said, "\n3 packets captured\n"));
	CHECK(holds(said, " packets received by filter\n0 packets dropped by kernel\n"));

	// A snap length that cuts the UDP header.
	CHECK(tcpdump_while("127.0.0.1", 7810, "-i lo -n -s 36 -c 1 udp dst port 7810", said,
				  sizeof(said))
			== 0);
	CHECK(holds(said, "snapshot length 36 bytes\n"));
	CHECK(holds(said, "IP 127.0.0.1 > 127.0.0.1:  [|udp]\n"));

	// Pings, both ways on lo, by a filter of ICMP.
	CHECK(output("ping -c 4 127.0.0.1 >/dev/null & timeout 30 tcpdump -i lo -n -c 2 icmp", said,
				  sizeof(said))
			== 0);
	CHECK(holds(said, "IP 127.0.0.1 > 127.0.0.1: ICMP echo request, id "));
	CHECK(holds(said, "IP 127.0.0.1 > 127.0.0.1: ICMP echo reply, id "));
	printf("capturetest: tcpdump read lo's datagrams and pings, filtered by the kernel\n");
}

static void tcpdump_any(void) {
	static char said[16384];
	CHECK(tcpdump_while("127.0.0.1", 7811, "-i any -n -c 2 udp dst port 7811", said,
				  sizeof(said))
			== 0);
	CHECK(holds(said, "listening on any, link-type LINUX_SLL2 (Linux cooked v2), snapshot length "
					  "262144 bytes\n"));
	CHECK(times(said, " lo    In  IP 127.0.0.1.") == 2);
	CHECK(times(said, " > 127.0.0.1.7811: UDP, length 12\n") == 2);
	CHECK(tcpdump_while("127.0.0.1", 7811, "-i any -y LINUX_SLL -n -e -c 1 udp dst port 7811",
				  said, sizeof(said))
			== 0);
	CHECK(holds(said, "listening on any, link-type LINUX_SLL (Linux cooked v1), snapshot length "
					  "262144 bytes\n"));
	CHECK(holds(said, " In 00:00:00:00:00:00 ethertype IPv4 (0x0800), length 56: 127.0.0.1."));
	CHECK(tcpdump_while("10.0.2.2", 7812, "-i any -n -c 1 udp dst port 7812", said, sizeof(said))
			== 0);
	CHECK(holds(said, " net0  Out IP 10.0.2.15."));
	CHECK(holds(said, " > 10.0.2.2.7812: UDP, length 12\n"));
	printf("capturetest: tcpdump read every interface's frames, cooked as on Linux\n");
}

static void tcpdump_files(void) {
	static char said[16384];
	CHECK(tcpdump_while("127.0.0.1", 7813,
				  "-i lo -n -c 2 -w /tmp/capturetest.pcap udp dst port 7813", said,
				  sizeof(said))
			== 0);
	CHECK(holds(said, "\n2 packets captured\n"));
	CHECK(output("tcpdump -n -r /tmp/capturetest.pcap", said, sizeof(said)) == 0);
	CHECK(holds(said, "reading from file /tmp/capturetest.pcap, link-type EN10MB (Ethernet), "
					  "snapshot length 262144\n"));
	CHECK(times(said, " > 127.0.0.1.7813: UDP, length 12\n") == 2);
	CHECK(output("tcpdump -n -r /tmp/capturetest.pcap udp dst port 9", said, sizeof(said)) == 0);
	CHECK(times(said, "UDP") == 0);
	uint8_t magic[4] = {0};
	FILE *file = fopen("/tmp/capturetest.pcap", "rb");
	CHECK(file && fread(magic, 1, 4, file) == 4);
	if(file)
		fclose(file);
	CHECK(!memcmp(magic, "\xd4\xc3\xb2\xa1", 4));

	CHECK(tcpdump_while("127.0.0.1", 7813,
				  "-i lo -n -c 1 --time-stamp-precision=nano -w /tmp/capturetest-nano.pcap "
				  "udp dst port 7813",
				  said, sizeof(said))
			== 0);
	file = fopen("/tmp/capturetest-nano.pcap", "rb");
	CHECK(file && fread(magic, 1, 4, file) == 4);
	if(file)
		fclose(file);
	CHECK(!memcmp(magic, "\x4d\x3c\xb2\xa1", 4));
	CHECK(output("tcpdump -n -tt --time-stamp-precision=nano -r /tmp/capturetest-nano.pcap", said,
				  sizeof(said))
			== 0);
	// Its time, by -tt: seconds, a point and nine digits.
	const char *ip = strstr(said, " IP 127.0.0.1.");
	CHECK(ip && ip - said >= 10 && ip[-10] == '.' && strspn(ip - 9, "0123456789") == 9);
	printf("capturetest: tcpdump wrote a capture file, by microseconds and nanoseconds, and "
		   "read it back\n");
}

static void tcpdump_tunnel(void) {
	static char said[16384];
	inject_port = 7815;
	CHECK(tcpdump_while("10.78.9.9", 7814, "-i " TUNNEL " -n -Q out -c 3 udp", said, sizeof(said))
			== 0);
	CHECK(holds(said, "listening on " TUNNEL ", link-type RAW (Raw IP), snapshot length 262144 "
					  "bytes\n"));
	CHECK(times(said, " > 10.78.9.9.7814: UDP, length 12\n") == 3);
	CHECK(times(said, "10.78.9.9.7815 >") == 0);
	CHECK(tcpdump_while("10.78.9.9", 7814, "-i " TUNNEL " -n -Q in -c 3 udp", said, sizeof(said))
			== 0);
	CHECK(times(said, "IP 10.78.9.9.7815 > 10.78.0.2.7815: UDP, length 12\n") == 3);
	CHECK(times(said, "> 10.78.9.9.7814") == 0);
	inject_port = 0;
	printf("capturetest: tcpdump read " TUNNEL "'s packets, each way alone\n");
}

static void tcpdump_card(void) {
	static char said[16384];
	CHECK(output("(sleep 1; echo capturetest | nc -w 2 10.0.2.100 7 >/dev/null) & "
				 "timeout 30 tcpdump -i net0 -n -c 3 tcp port 7",
				  said, sizeof(said))
			== 0);
	CHECK(holds(said, "tcpdump: WARNING: net0: That device doesn't support promiscuous mode\n"
					  "(Dukovo cannot set a network card to promiscuous mode yet)\n"));
	CHECK(holds(said, "listening on net0, link-type EN10MB (Ethernet), snapshot length 262144 "
					  "bytes\n"));
	CHECK(holds(said, "IP 10.0.2.15."));
	CHECK(holds(said, " > 10.0.2.100.7: Flags [S], seq "));
	CHECK(holds(said, "IP 10.0.2.100.7 > 10.0.2.15."));
	CHECK(holds(said, ": Flags [S.], seq "));
	CHECK(tcpdump_while("10.0.2.2", 7812, "-i net0 -p -n -e -c 1 udp dst port 7812", said,
				  sizeof(said))
			== 0);
	CHECK(times(said, "promiscuous") == 0);
	CHECK(holds(said, " 52:54:00:12:34:56 > "));
	CHECK(holds(said, ", ethertype IPv4 (0x0800), length 54: 10.0.2.15."));
	printf("capturetest: tcpdump read net0's frames, TCP's handshake among them\n");
}

// tcpdump stopped by SIGINT says what it counted, as on Linux.
static void tcpdump_interrupted(void) {
	pid_t sender = sending("127.0.0.1", 7816);
	pid_t pid = check_fork();
	if(pid == 0) {
		execl("/bin/sh", "sh", "-c",
				"exec tcpdump -i lo -n udp dst port 7816 >/tmp/capturetest-int 2>&1",
				(char *)NULL);
		_exit(127);
	}
	static char said[16384];
	int seen = 0;
	for(int i = 0; i < 100 && !seen; i++) {
		check_sleep_ms(100);
		FILE *file = fopen("/tmp/capturetest-int", "r");
		if(!file)
			continue;
		size_t length = fread(said, 1, sizeof(said) - 1, file);
		said[length] = 0;
		fclose(file);
		seen = times(said, " > 127.0.0.1.7816: UDP, length 12\n") >= 2;
	}
	CHECK(seen);
	CHECK(kill(pid, SIGINT) == 0);
	int status = check_wait(pid);
	CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
	stop(sender);
	FILE *file = fopen("/tmp/capturetest-int", "r");
	CHECK(file != NULL);
	if(file) {
		size_t length = fread(said, 1, sizeof(said) - 1, file);
		said[length] = 0;
		fclose(file);
	}
	fputs(said, stdout);
	CHECK(holds(said, " packets captured\n"));
	CHECK(holds(said, " packets received by filter\n0 packets dropped by kernel\n"));
	printf("capturetest: tcpdump stopped at SIGINT and said what it counted\n");
}

int main(void) {
	check_start("capturetest");
	signal(SIGPIPE, SIG_IGN);
	RUN(setup);
	RUN(frames_of_lo);
	RUN(limits);
	RUN(filters);
	RUN(drops);
	RUN(tunnels);
	RUN(gone);
	RUN(rights);
	RUN(most);
	RUN(tcpdump_lists);
	RUN(tcpdump_lo);
	RUN(tcpdump_any);
	RUN(tcpdump_files);
	RUN(tcpdump_tunnel);
	RUN(tcpdump_card);
	RUN(tcpdump_interrupted);
	stopping = 1;
	return check_end();
}
