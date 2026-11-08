// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// AF_INET sockets on the kernel's TCP and UDP (ADR 0183), over the
// loopback interface. TCP: bind, listen, connect and accept, their
// addresses, data both ways, shutdown and the end of the stream, the
// options, a connection refused, non-blocking connect and accept with
// poll, a listener shared with a child, a socket that listens unbound,
// more connections at once than an interface's first socket slots, and
// a connection made while a thread waits in accept and another opens a
// descriptor.
// UDP: sendto and recvfrom, datagrams kept whole, sendmsg and recvmsg
// over several buffers, connect, and MSG_DONTWAIT. The errors of
// addresses in use or not on the machine and of families not there.
// AF_INET6 over ::1 (ADR 0332): a connection, its addresses and its
// data, IPV6_V6ONLY, and an IPv4 client reaching a socket bound to ::
// as ::ffff:127.0.0.1. And
// the interfaces' names and numbers (if_nametoindex, if_indextoname,
// if_nameindex) and SO_BINDTODEVICE, of lo, the only interface this boot
// has; the interfaces boot test binds to a card. And
// ping sockets, IP_PKTINFO and IPV6_RECVPKTINFO, ports shared with
// SO_REUSEPORT, multicast groups on lo, and getifaddrs (ADR 0336).
// MSG_PEEK on streams, datagrams and ping sockets, and getnameinfo's
// numeric names with their scopes (ADR 0341). ICMP errors (ADR 0345):
// a connected UDP socket's ECONNREFUSED, by recv, send, poll's POLLERR
// and SO_ERROR, an unconnected one hearing nothing, and IP_RECVERR's and
// IPV6_RECVERR's queue of errors, read with MSG_ERRQUEUE, and IP_TOS. And
// the network capabilities: a child with none cannot make a socket
// (EACCES), one that may only connect out cannot bind a port or listen,
// or join or send to a group, one with the local right alone reaches
// this machine and joins groups (ADR 0337), and one that may only listen
// on ports 9000 to 9009 cannot connect out or bind another port. The
// runner gets Spark's network capability, and this test gets it through
// fork and exec.

#include <arpa/inet.h>
#include <fcntl.h>
#include <ifaddrs.h>
#include <linux/errqueue.h>
#include <net/if.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <pthread.h>
#include <radix/radix.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/uio.h>

#include "check.h"

// 127.0.0.1 and `port`.
static struct sockaddr_in loopback(unsigned short port) {
	struct sockaddr_in in;
	memset(&in, 0, sizeof(in));
	in.sin_family = AF_INET;
	in.sin_port = htons(port);
	in.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
	return in;
}

// The port socket `fd` is bound to.
static unsigned short port_of(int fd) {
	struct sockaddr_in in;
	socklen_t len = sizeof(in);
	CHECK(getsockname(fd, (struct sockaddr *)&in, &len) == 0);
	CHECK(len == sizeof(in) && in.sin_family == AF_INET);
	return ntohs(in.sin_port);
}

// A TCP socket listening on 127.0.0.1, at a port the kernel picks.
static int listener(void) {
	int fd = socket(AF_INET, SOCK_STREAM, 0);
	CHECK(fd >= 0);
	struct sockaddr_in at = loopback(0);
	CHECK(bind(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	CHECK(listen(fd, 4) == 0);
	return fd;
}

// A TCP socket connected to 127.0.0.1 `port`.
static int connected(unsigned short port) {
	int fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
	CHECK(fd >= 0);
	struct sockaddr_in to = loopback(port);
	CHECK(connect(fd, (struct sockaddr *)&to, sizeof(to)) == 0);
	return fd;
}

// The order the benchmarks use: the port read right after bind, and
// TCP_NODELAY set before the connection.
static void bench_order(void) {
	int fd = socket(AF_INET, SOCK_STREAM, 0);
	CHECK(fd >= 0);
	struct sockaddr_in at = loopback(0);
	socklen_t length = sizeof(at);
	CHECK(bind(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	CHECK(getsockname(fd, (struct sockaddr *)&at, &length) == 0);
	unsigned short port = ntohs(at.sin_port);
	printf("inet: bound to port %u\n", port);
	CHECK(listen(fd, 16) == 0);
	CHECK(port_of(fd) == port);
	int client = socket(AF_INET, SOCK_STREAM, 0);
	int on = 1;
	CHECK(setsockopt(client, IPPROTO_TCP, TCP_NODELAY, &on, sizeof(on)) == 0);
	struct sockaddr_in to = loopback(port);
	CHECK(connect(client, (struct sockaddr *)&to, sizeof(to)) == 0);
	int accepted = accept(fd, NULL, NULL);
	CHECK(accepted >= 0);
	close(client);
	close(accepted);
	close(fd);
}

// More short connections in a row than the loopback interface has
// sockets: each ends at once, in TIME-WAIT, and gives its socket back.
static void short_connections(void) {
	int server = listener();
	unsigned short port = port_of(server);
	for(int i = 0; i < 300; i++) {
		int client = connected(port);
		int accepted = accept(server, NULL, NULL);
		CHECK(accepted >= 0);
		CHECK(close(client) == 0);
		CHECK(close(accepted) == 0);
	}
	CHECK(close(server) == 0);
}

// More sockets at once than the loopback interface has slots for at
// first (128), two for each connection and the listener: the interface
// makes more slots (ADR 0188), and the connections made first still
// carry bytes.
static void many_connections(void) {
	enum { COUNT = 70 };
	int server = listener();
	unsigned short port = port_of(server);
	int clients[COUNT], accepted[COUNT];
	for(int i = 0; i < COUNT; i++) {
		clients[i] = connected(port);
		accepted[i] = accept(server, NULL, NULL);
		CHECK(accepted[i] >= 0);
	}
	for(int i = 0; i < COUNT; i++) {
		unsigned char byte = (unsigned char)i;
		CHECK(write(clients[i], &byte, 1) == 1);
		unsigned char got = 0;
		CHECK(read(accepted[i], &got, 1) == 1 && got == byte);
		CHECK(close(clients[i]) == 0);
		CHECK(close(accepted[i]) == 0);
	}
	CHECK(close(server) == 0);
}

// A listener, and what a thread that waits in accept on it took.
struct waiter {
	int server;
	int accepted;
};

static void *accept_one(void *arg) {
	struct waiter *waiter = arg;
	waiter->accepted = accept(waiter->server, NULL, NULL);
	return NULL;
}

// A thread that waits in accept gets the connection made while it waits,
// even when another thread's socket took the descriptor accept was to
// fill, the lowest free one: the kernel gives the connection back to the
// listener, and accept takes it into the next free descriptor. It was
// closed, and accept waited on (rustls's handshakes in the tls boot test
// found it).
static void accept_while_another_opens(void) {
	int server = listener();
	unsigned short port = port_of(server);
	// Ten seconds at most, so that a lost connection fails the check.
	struct timeval patience = {.tv_sec = 10};
	CHECK(setsockopt(server, SOL_SOCKET, SO_RCVTIMEO, &patience, sizeof(patience)) == 0);
	for(int round = 0; round < 3; round++) {
		struct waiter waiter = {.server = server, .accepted = -1};
		pthread_t thread;
		CHECK(pthread_create(&thread, NULL, accept_one, &waiter) == 0);
		// Long enough for the thread to wait in accept.
		struct timespec pause = {.tv_nsec = 50000000};
		while(nanosleep(&pause, &pause) != 0 && errno == EINTR) {}
		int client = connected(port);
		CHECK(pthread_join(thread, NULL) == 0);
		CHECK(waiter.accepted >= 0 && waiter.accepted != client);
		char byte = 'a';
		CHECK(write(client, &byte, 1) == 1);
		CHECK(read(waiter.accepted, &byte, 1) == 1 && byte == 'a');
		CHECK(write(waiter.accepted, "b", 1) == 1);
		CHECK(read(client, &byte, 1) == 1 && byte == 'b');
		CHECK(close(client) == 0);
		CHECK(close(waiter.accepted) == 0);
	}
	CHECK(close(server) == 0);
}

static void tcp(void) {
	int server = listener();
	unsigned short port = port_of(server);
	CHECK(port != 0);
	int value = 0;
	socklen_t size = sizeof(value);
	CHECK(getsockopt(server, SOL_SOCKET, SO_ACCEPTCONN, &value, &size) == 0 && value == 1);
	int client = connected(port);
	struct sockaddr_in peer;
	socklen_t len = sizeof(peer);
	int accepted = accept(server, (struct sockaddr *)&peer, &len);
	CHECK(accepted >= 0);
	// The accepted connection came from the client's port, which the
	// kernel picked when it connected.
	CHECK(len == sizeof(peer) && peer.sin_family == AF_INET);
	CHECK(peer.sin_addr.s_addr == htonl(INADDR_LOOPBACK));
	unsigned short from = port_of(client);
	CHECK(ntohs(peer.sin_port) == from && from >= 49152);
	len = sizeof(peer);
	CHECK(getpeername(client, (struct sockaddr *)&peer, &len) == 0);
	CHECK(ntohs(peer.sin_port) == port);
	CHECK(port_of(accepted) == port);
	// Bytes both ways, with write and read, send and recv.
	CHECK(write(client, "ping", 4) == 4);
	char buffer[64] = {0};
	CHECK(read(accepted, buffer, sizeof(buffer)) == 4 && !memcmp(buffer, "ping", 4));
	CHECK(send(accepted, "pong!", 5, 0) == 5);
	CHECK(recv(client, buffer, 5, MSG_WAITALL) == 5 && !memcmp(buffer, "pong!", 5));
	// recvfrom on a stream says nothing of where the bytes came from.
	CHECK(send(accepted, "abc", 3, 0) == 3);
	struct sockaddr_in where;
	socklen_t where_len = sizeof(where);
	CHECK(recvfrom(client, buffer, sizeof(buffer), 0, (struct sockaddr *)&where, &where_len)
			== 3);
	CHECK(where_len == 0);
	// What getsockopt says, and the options it takes.
	CHECK(getsockopt(client, SOL_SOCKET, SO_TYPE, &value, &size) == 0 && value == SOCK_STREAM);
	CHECK(getsockopt(client, SOL_SOCKET, SO_DOMAIN, &value, &size) == 0 && value == AF_INET);
	CHECK(getsockopt(client, SOL_SOCKET, SO_PROTOCOL, &value, &size) == 0
			&& value == IPPROTO_TCP);
	CHECK(getsockopt(client, SOL_SOCKET, SO_ERROR, &value, &size) == 0 && value == 0);
	value = 1;
	CHECK(setsockopt(client, IPPROTO_TCP, TCP_NODELAY, &value, sizeof(value)) == 0);
	CHECK(getsockopt(client, IPPROTO_TCP, TCP_NODELAY, &value, &size) == 0 && value == 1);
	value = 1;
	CHECK(setsockopt(client, SOL_SOCKET, SO_KEEPALIVE, &value, sizeof(value)) == 0);
	CHECK(setsockopt(server, SOL_SOCKET, SO_REUSEADDR, &value, sizeof(value)) == 0);
	// Keep-alive's probes as on Linux: 7,200 s of quiet, then 9, 75 s apart.
	CHECK(getsockopt(client, IPPROTO_TCP, TCP_KEEPIDLE, &value, &size) == 0 && value == 7200);
	CHECK(getsockopt(client, IPPROTO_TCP, TCP_KEEPINTVL, &value, &size) == 0 && value == 75);
	CHECK(getsockopt(client, IPPROTO_TCP, TCP_KEEPCNT, &value, &size) == 0 && value == 9);
	value = 60;
	CHECK(setsockopt(client, IPPROTO_TCP, TCP_KEEPIDLE, &value, sizeof(value)) == 0);
	CHECK(getsockopt(client, IPPROTO_TCP, TCP_KEEPIDLE, &value, &size) == 0 && value == 60);
	value = 0;
	CHECK_ERROR(setsockopt(client, IPPROTO_TCP, TCP_KEEPINTVL, &value, sizeof(value)), EINVAL);
	value = 128;
	CHECK_ERROR(setsockopt(client, IPPROTO_TCP, TCP_KEEPCNT, &value, sizeof(value)), EINVAL);
	value = 3;
	CHECK(setsockopt(client, IPPROTO_TCP, TCP_KEEPCNT, &value, sizeof(value)) == 0);
	value = 32;
	CHECK(setsockopt(client, IPPROTO_IP, IP_TTL, &value, sizeof(value)) == 0);
	CHECK(getsockopt(client, IPPROTO_IP, IP_TTL, &value, &size) == 0 && value == 32);
	struct timeval tv = {2, 500000};
	CHECK(setsockopt(client, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv)) == 0);
	memset(&tv, 0, sizeof(tv));
	socklen_t tv_size = sizeof(tv);
	CHECK(getsockopt(client, SOL_SOCKET, SO_RCVTIMEO, &tv, &tv_size) == 0);
	CHECK(tv.tv_sec == 2 && tv.tv_usec == 500000);
	CHECK_ERROR(setsockopt(client, SOL_SOCKET, SO_PEERCRED, &value, sizeof(value)), ENOPROTOOPT);
	// FIONREAD counts what waits.
	CHECK(write(accepted, "12345", 5) == 5);
	check_sleep_ms(20);
	int readable = -1;
	CHECK(ioctl(client, FIONREAD, &readable) == 0 && readable == 5);
	CHECK(read(client, buffer, sizeof(buffer)) == 5);
	// The client stops sending: the other end reads the end, and can
	// still send back.
	CHECK(shutdown(client, SHUT_WR) == 0);
	CHECK(read(accepted, buffer, sizeof(buffer)) == 0);
	CHECK(write(accepted, "last", 4) == 4);
	CHECK(read(client, buffer, sizeof(buffer)) == 4 && !memcmp(buffer, "last", 4));
	close(accepted);
	CHECK(read(client, buffer, sizeof(buffer)) == 0);
	close(client);
	close(server);
	// Nothing listens there now.
	int refused = socket(AF_INET, SOCK_STREAM, 0);
	struct sockaddr_in to = loopback(port);
	CHECK_ERROR(connect(refused, (struct sockaddr *)&to, sizeof(to)), ECONNREFUSED);
	close(refused);
}

static void non_blocking(void) {
	int server = listener();
	CHECK(fcntl(server, F_SETFL, O_NONBLOCK) == 0);
	CHECK_ERROR(accept(server, NULL, NULL), EAGAIN);
	int client = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
	CHECK(client >= 0);
	CHECK((fcntl(client, F_GETFD) & FD_CLOEXEC) && (fcntl(client, F_GETFL) & O_NONBLOCK));
	struct sockaddr_in to = loopback(port_of(server));
	CHECK_ERROR(connect(client, (struct sockaddr *)&to, sizeof(to)), EINPROGRESS);
	// Writable once connected, with no error.
	struct pollfd p = {client, POLLOUT, 0};
	CHECK(poll(&p, 1, 5000) == 1 && (p.revents & POLLOUT));
	int value = -1;
	socklen_t size = sizeof(value);
	CHECK(getsockopt(client, SOL_SOCKET, SO_ERROR, &value, &size) == 0 && value == 0);
	CHECK_ERROR(connect(client, (struct sockaddr *)&to, sizeof(to)), EISCONN);
	// The listener is readable with a connection to take.
	struct pollfd q = {server, POLLIN, 0};
	CHECK(poll(&q, 1, 5000) == 1 && (q.revents & POLLIN));
	int accepted = accept4(server, NULL, NULL, SOCK_NONBLOCK);
	CHECK(accepted >= 0);
	char buffer[8];
	CHECK_ERROR(read(accepted, buffer, sizeof(buffer)), EAGAIN);
	CHECK_ERROR(recv(client, buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	CHECK(send(client, "x", 1, 0) == 1);
	struct pollfd r = {accepted, POLLIN, 0};
	CHECK(poll(&r, 1, 5000) == 1 && (r.revents & POLLIN));
	CHECK(read(accepted, buffer, sizeof(buffer)) == 1);
	close(accepted);
	close(client);
	close(server);
}

static void unbound(void) {
	// A socket that listens unbound is bound to a port the kernel picks,
	// as on Linux.
	int server = socket(AF_INET, SOCK_STREAM, 0);
	CHECK(listen(server, 1) == 0);
	unsigned short port = port_of(server);
	CHECK(port >= 49152);
	int client = connected(port);
	int accepted = accept(server, NULL, NULL);
	CHECK(accepted >= 0);
	close(accepted);
	close(client);
	close(server);
}

static void shared(void) {
	// A child, which has the network capabilities through fork, connects
	// to the parent's listener.
	int server = listener();
	unsigned short port = port_of(server);
	pid_t pid = check_fork();
	if(pid == 0) {
		int client = connected(port);
		CHECK(write(client, "from child", 10) == 10);
		close(client);
		check_exit_child();
	}
	int accepted = accept(server, NULL, NULL);
	CHECK(accepted >= 0);
	char buffer[16] = {0};
	size_t got = 0;
	ssize_t n;
	while((n = read(accepted, buffer + got, sizeof(buffer) - got)) > 0)
		got += (size_t)n;
	CHECK(got == 10 && !memcmp(buffer, "from child", 10));
	CHECK_CHILD(pid);
	close(accepted);
	close(server);
}

static void udp(void) {
	int a = socket(AF_INET, SOCK_DGRAM, 0);
	int b = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
	CHECK(a >= 0 && b >= 0);
	struct sockaddr_in at = loopback(0);
	CHECK(bind(a, (struct sockaddr *)&at, sizeof(at)) == 0);
	CHECK(bind(b, (struct sockaddr *)&at, sizeof(at)) == 0);
	struct sockaddr_in to_a = loopback(port_of(a));
	struct sockaddr_in to_b = loopback(port_of(b));
	int value = 0;
	socklen_t size = sizeof(value);
	CHECK(getsockopt(a, SOL_SOCKET, SO_TYPE, &value, &size) == 0 && value == SOCK_DGRAM);
	CHECK(getsockopt(a, SOL_SOCKET, SO_PROTOCOL, &value, &size) == 0 && value == IPPROTO_UDP);
	// Two datagrams, each received whole and alone, with who sent it.
	CHECK(sendto(a, "first", 5, 0, (struct sockaddr *)&to_b, sizeof(to_b)) == 5);
	CHECK(sendto(a, "second!", 7, 0, (struct sockaddr *)&to_b, sizeof(to_b)) == 7);
	char buffer[64];
	struct sockaddr_in from;
	socklen_t from_len = sizeof(from);
	CHECK(recvfrom(b, buffer, sizeof(buffer), 0, (struct sockaddr *)&from, &from_len) == 5);
	CHECK(!memcmp(buffer, "first", 5) && from_len == sizeof(from));
	CHECK(ntohs(from.sin_port) == port_of(a) && from.sin_addr.s_addr == htonl(INADDR_LOOPBACK));
	CHECK(recv(b, buffer, sizeof(buffer), 0) == 7 && !memcmp(buffer, "second!", 7));
	CHECK_ERROR(recv(b, buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	// sendmsg gathers its buffers into one datagram; recvmsg spreads one
	// over its buffers.
	struct iovec out[2] = {{(void *)"gat", 3}, {(void *)"hered", 5}};
	struct msghdr hdr;
	memset(&hdr, 0, sizeof(hdr));
	hdr.msg_name = &to_a;
	hdr.msg_namelen = sizeof(to_a);
	hdr.msg_iov = out;
	hdr.msg_iovlen = 2;
	CHECK(sendmsg(b, &hdr, 0) == 8);
	char first[4], second[8];
	struct iovec in[2] = {{first, sizeof(first)}, {second, sizeof(second)}};
	memset(&hdr, 0, sizeof(hdr));
	hdr.msg_name = &from;
	hdr.msg_namelen = sizeof(from);
	hdr.msg_iov = in;
	hdr.msg_iovlen = 2;
	CHECK(recvmsg(a, &hdr, 0) == 8);
	CHECK(!memcmp(first, "gath", 4) && !memcmp(second, "ered", 4));
	CHECK(hdr.msg_namelen == sizeof(from) && ntohs(from.sin_port) == port_of(b));
	// Connected, send and recv need no address.
	CHECK(connect(a, (struct sockaddr *)&to_b, sizeof(to_b)) == 0);
	struct sockaddr_in peer;
	socklen_t len = sizeof(peer);
	CHECK(getpeername(a, (struct sockaddr *)&peer, &len) == 0 && ntohs(peer.sin_port) == port_of(b));
	CHECK(send(a, "linked", 6, 0) == 6);
	CHECK(read(b, buffer, sizeof(buffer)) == 6 && !memcmp(buffer, "linked", 6));
	// A datagram socket is writable, and readable once one waits.
	struct pollfd p = {b, POLLIN | POLLOUT, 0};
	CHECK(poll(&p, 1, 0) == 1 && p.revents == POLLOUT);
	// The loopback interface's thread hands the datagram over after send
	// returns, so poll waits for it.
	CHECK(send(a, "!", 1, 0) == 1);
	p.events = POLLIN;
	CHECK(poll(&p, 1, 5000) == 1 && (p.revents & POLLIN));
	CHECK(recv(b, buffer, sizeof(buffer), 0) == 1);
	close(a);
	close(b);
}

// Waits up to 5 s for `fd` to have `count` bytes or more to read.
static void waiting_for(int fd, int count) {
	int waiting = 0;
	for(int i = 0; i < 5000; i++) {
		CHECK(ioctl(fd, FIONREAD, &waiting) == 0);
		if(waiting >= count)
			return;
		check_sleep_ms(1);
	}
	CHECK(waiting >= count);
}

// MSG_PEEK (ADR 0341): what a peek gets stays for the next receive.
static void peeking(void) {
	int server = listener();
	int client = connected(port_of(server));
	int accepted = accept(server, NULL, NULL);
	CHECK(accepted >= 0);
	static char buffer[4096], want[4096];
	CHECK_ERROR(recv(client, buffer, sizeof(buffer), MSG_PEEK | MSG_DONTWAIT), EAGAIN);
	CHECK(send(accepted, "peekaboo", 8, 0) == 8);
	waiting_for(client, 8);
	CHECK(recv(client, buffer, 4, MSG_PEEK) == 4 && !memcmp(buffer, "peek", 4));
	CHECK(recv(client, buffer, sizeof(buffer), MSG_PEEK) == 8 && !memcmp(buffer, "peekaboo", 8));
	char first[3], second[8];
	struct iovec spread[2] = {{first, sizeof(first)}, {second, sizeof(second)}};
	struct msghdr hdr = {.msg_iov = spread, .msg_iovlen = 2};
	CHECK(recvmsg(client, &hdr, MSG_PEEK) == 8);
	CHECK(!memcmp(first, "pee", 3) && !memcmp(second, "kaboo", 5));
	CHECK(recv(client, buffer, sizeof(buffer), 0) == 8 && !memcmp(buffer, "peekaboo", 8));
	// 3,000 bytes at a time, each peeked whole before it is read, until
	// twice round the receive buffer: some lie in two pieces of it.
	int size = 0;
	socklen_t len = sizeof(size);
	CHECK(getsockopt(client, SOL_SOCKET, SO_RCVBUF, &size, &len) == 0 && size > 0);
	size_t at = 0;
	while(at < 2 * (size_t)size + 3000) {
		for(size_t i = 0; i < 3000; i++)
			want[i] = (char)((at + i) * 7 + (at + i) / 251);
		CHECK(send(accepted, want, 3000, 0) == 3000);
		waiting_for(client, 3000);
		memset(buffer, 0, sizeof(buffer));
		if(recv(client, buffer, sizeof(buffer), MSG_PEEK) != 3000 || memcmp(buffer, want, 3000)) {
			CHECK(!"a peek saw the 3,000 bytes whole");
			break;
		}
		CHECK(recv(client, buffer, 3000, MSG_WAITALL) == 3000 && !memcmp(buffer, want, 3000));
		at += 3000;
	}
	// At the end of the stream a peek gets 0, as a read does.
	CHECK(shutdown(accepted, SHUT_WR) == 0);
	struct pollfd p = {client, POLLIN, 0};
	CHECK(poll(&p, 1, 5000) == 1);
	CHECK(recv(client, buffer, sizeof(buffer), MSG_PEEK) == 0);
	CHECK(recv(client, buffer, sizeof(buffer), 0) == 0);
	close(accepted);
	close(client);
	close(server);

	// A datagram stays whole, and says who sent it each time, as nc's
	// UDP listener asks before it connects.
	int a = socket(AF_INET, SOCK_DGRAM, 0);
	int b = socket(AF_INET, SOCK_DGRAM, 0);
	struct sockaddr_in any = loopback(0);
	CHECK(bind(a, (struct sockaddr *)&any, sizeof(any)) == 0);
	CHECK(bind(b, (struct sockaddr *)&any, sizeof(any)) == 0);
	struct sockaddr_in to_b = loopback(port_of(b));
	CHECK(sendto(a, "one", 3, 0, (struct sockaddr *)&to_b, sizeof(to_b)) == 3);
	CHECK(sendto(a, "two!", 4, 0, (struct sockaddr *)&to_b, sizeof(to_b)) == 4);
	p.fd = b;
	CHECK(poll(&p, 1, 5000) == 1);
	struct sockaddr_in from;
	for(int i = 0; i < 2; i++) {
		socklen_t from_len = sizeof(from);
		memset(&from, 0, sizeof(from));
		CHECK(recvfrom(b, buffer, sizeof(buffer), MSG_PEEK, (struct sockaddr *)&from, &from_len)
				== 3);
		CHECK(!memcmp(buffer, "one", 3) && ntohs(from.sin_port) == port_of(a));
	}
	CHECK(recv(b, buffer, 2, MSG_PEEK) == 2 && !memcmp(buffer, "on", 2));
	hdr.msg_name = &from;
	hdr.msg_namelen = sizeof(from);
	CHECK(recvmsg(b, &hdr, MSG_PEEK) == 3 && !memcmp(first, "one", 3));
	CHECK(hdr.msg_namelen == sizeof(from) && ntohs(from.sin_port) == port_of(a));
	CHECK(recv(b, buffer, sizeof(buffer), 0) == 3 && !memcmp(buffer, "one", 3));
	CHECK(recv(b, buffer, sizeof(buffer), MSG_PEEK | MSG_DONTWAIT) == 4);
	CHECK(recv(b, buffer, sizeof(buffer), MSG_DONTWAIT) == 4 && !memcmp(buffer, "two!", 4));
	CHECK_ERROR(recv(b, buffer, sizeof(buffer), MSG_PEEK | MSG_DONTWAIT), EAGAIN);
	close(a);
	close(b);

	// A ping socket's reply stays too.
	int ping = socket(AF_INET, SOCK_DGRAM, IPPROTO_ICMP);
	CHECK(ping >= 0);
	unsigned char request[12] = {8, 0, 0, 0, 0, 0, 0, 1, 'p', 'e', 'e', 'k'};
	CHECK(sendto(ping, request, sizeof(request), 0, (struct sockaddr *)&any, sizeof(any)) == 12);
	p.fd = ping;
	CHECK(poll(&p, 1, 5000) == 1);
	unsigned char reply[64];
	CHECK(recv(ping, reply, sizeof(reply), MSG_PEEK) == 12 && reply[0] == 0);
	CHECK(recv(ping, reply, sizeof(reply), 0) == 12 && !memcmp(reply + 8, "peek", 4));
	CHECK_ERROR(recv(ping, reply, sizeof(reply), MSG_DONTWAIT), EAGAIN);
	close(ping);
}

// getnameinfo's numeric names (ADR 0341): a scoped IPv6 address has
// its scope after a %, as glibc gives it, a link's interface by name.
static void numeric_names(void) {
	struct sockaddr_in four = loopback(80);
	char host[64], serv[16], want[64];
	int numeric = NI_NUMERICHOST | NI_NUMERICSERV;
	CHECK(getnameinfo((struct sockaddr *)&four, sizeof(four), host, sizeof(host), serv,
			sizeof(serv), numeric) == 0);
	CHECK(!strcmp(host, "127.0.0.1") && !strcmp(serv, "80"));
	struct sockaddr_in6 six;
	memset(&six, 0, sizeof(six));
	six.sin6_family = AF_INET6;
	six.sin6_port = htons(7000);
	CHECK(inet_pton(AF_INET6, "fe80::1", &six.sin6_addr) == 1);
	unsigned int lo = if_nametoindex("lo");
	CHECK(lo != 0);
	six.sin6_scope_id = lo;
	CHECK(getnameinfo((struct sockaddr *)&six, sizeof(six), host, sizeof(host), serv,
			sizeof(serv), numeric) == 0);
	CHECK(!strcmp(host, "fe80::1%lo") && !strcmp(serv, "7000"));
	CHECK(getnameinfo((struct sockaddr *)&six, sizeof(six), host, sizeof(host), NULL, 0,
			numeric | NI_NUMERICSCOPE) == 0);
	snprintf(want, sizeof(want), "fe80::1%%%u", lo);
	CHECK(!strcmp(host, want));
	// No room for the scope.
	CHECK(getnameinfo((struct sockaddr *)&six, sizeof(six), host, 9, NULL, 0, numeric)
			== EAI_OVERFLOW);
	// An interface that is gone, and another scope, are numbers.
	six.sin6_scope_id = 99;
	CHECK(getnameinfo((struct sockaddr *)&six, sizeof(six), host, sizeof(host), NULL, 0,
			numeric) == 0 && !strcmp(host, "fe80::1%99"));
	CHECK(inet_pton(AF_INET6, "2001:db8::1", &six.sin6_addr) == 1);
	six.sin6_scope_id = 5;
	CHECK(getnameinfo((struct sockaddr *)&six, sizeof(six), host, sizeof(host), NULL, 0,
			numeric) == 0 && !strcmp(host, "2001:db8::1%5"));
	six.sin6_scope_id = 0;
	CHECK(getnameinfo((struct sockaddr *)&six, sizeof(six), host, sizeof(host), NULL, 0,
			numeric) == 0 && !strcmp(host, "2001:db8::1"));
}

static void errors(void) {
	int a = socket(AF_INET, SOCK_STREAM, 0);
	int b = socket(AF_INET, SOCK_STREAM, 0);
	struct sockaddr_in at = loopback(0);
	CHECK(bind(a, (struct sockaddr *)&at, sizeof(at)) == 0);
	// One socket to a port.
	at = loopback(port_of(a));
	CHECK_ERROR(bind(b, (struct sockaddr *)&at, sizeof(at)), EADDRINUSE);
	// The port stays the first one's after a bind is refused it.
	CHECK_ERROR(bind(b, (struct sockaddr *)&at, sizeof(at)), EADDRINUSE);
	// An address the machine does not have.
	struct sockaddr_in away = loopback(0);
	away.sin_addr.s_addr = inet_addr("10.9.9.9");
	CHECK_ERROR(bind(b, (struct sockaddr *)&away, sizeof(away)), EADDRNOTAVAIL);
	// An address too short, and one of another family.
	CHECK_ERROR(bind(b, (struct sockaddr *)&at, sizeof(at) - 1), EINVAL);
	struct sockaddr_in other = at;
	other.sin_family = AF_UNIX;
	CHECK_ERROR(connect(b, (struct sockaddr *)&other, sizeof(other)), EAFNOSUPPORT);
	close(a);
	close(b);
	CHECK_ERROR(socket(AF_PACKET, SOCK_RAW, 0), EAFNOSUPPORT);
	CHECK_ERROR(socket(AF_INET, SOCK_STREAM, IPPROTO_UDP), EPROTONOSUPPORT);
	CHECK_ERROR(socket(AF_INET, SOCK_RAW, IPPROTO_ICMP), EPERM);
	// A datagram goes somewhere.
	int d = socket(AF_INET, SOCK_DGRAM, 0);
	CHECK_ERROR(send(d, "x", 1, 0), ENOTCONN);
	close(d);
}

// A port of 127.0.0.1's that no socket has.
static unsigned short closed_port(void) {
	int fd = socket(AF_INET, SOCK_DGRAM, 0);
	struct sockaddr_in at = loopback(0);
	CHECK(fd >= 0 && bind(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	unsigned short port = port_of(fd);
	close(fd);
	return port;
}

// Waits up to 5 s for an error on `fd`.
static void waiting_for_error(int fd) {
	struct pollfd p = {fd, POLLIN, 0};
	CHECK(poll(&p, 1, 5000) == 1 && (p.revents & POLLERR));
}

// The error IP_RECVERR or IPV6_RECVERR queued on `fd`, with what it
// quotes in `data` (its length the result) and where it was about in
// `to`.
struct queued {
	struct sock_extended_err ee;
	union {
		struct sockaddr_in in;
		struct sockaddr_in6 in6;
	} offender;
};

static ssize_t queued_error(int fd, char *data, size_t size, void *to, socklen_t *to_len,
		int level, int type, struct queued *error, int *flags) {
	struct iovec iov = {data, size};
	union {
		struct cmsghdr align;
		char space[CMSG_SPACE(sizeof(struct queued))];
	} control;
	struct msghdr hdr;
	memset(&hdr, 0, sizeof(hdr));
	hdr.msg_name = to;
	hdr.msg_namelen = *to_len;
	hdr.msg_iov = &iov;
	hdr.msg_iovlen = 1;
	hdr.msg_control = control.space;
	hdr.msg_controllen = sizeof(control.space);
	ssize_t got = recvmsg(fd, &hdr, MSG_ERRQUEUE);
	if(got < 0)
		return got;
	*to_len = hdr.msg_namelen;
	*flags = hdr.msg_flags;
	struct cmsghdr *c = CMSG_FIRSTHDR(&hdr);
	CHECK(c && c->cmsg_level == level && c->cmsg_type == type);
	CHECK(c->cmsg_len >= CMSG_LEN(sizeof(struct sock_extended_err)));
	memset(error, 0, sizeof(*error));
	memcpy(error, CMSG_DATA(c), c->cmsg_len - CMSG_LEN(0));
	CHECK(!CMSG_NXTHDR(&hdr, c));
	return got;
}

// ICMP errors (ADR 0345). A connected UDP socket learns that nothing
// listens where it sends, as on Linux: its next recv or send fails with
// ECONNREFUSED, once, and poll says POLLERR until then, or SO_ERROR
// takes it. An unconnected socket hears of it only with IP_RECVERR,
// which queues each error with what it was about, for MSG_ERRQUEUE.
static void icmp_errors(void) {
	char buffer[64];
	unsigned short closed = closed_port();
	struct sockaddr_in there = loopback(closed);
	int fd = socket(AF_INET, SOCK_DGRAM, 0);
	CHECK(fd >= 0);
	CHECK(connect(fd, (struct sockaddr *)&there, sizeof(there)) == 0);
	CHECK(send(fd, "probe", 5, 0) == 5);
	waiting_for_error(fd);
	CHECK_ERROR(recv(fd, buffer, sizeof(buffer), 0), ECONNREFUSED);
	CHECK_ERROR(recv(fd, buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	// The error fails a send too, and SO_ERROR takes it as well.
	CHECK(send(fd, "probe", 5, 0) == 5);
	waiting_for_error(fd);
	CHECK_ERROR(send(fd, "again", 5, 0), ECONNREFUSED);
	CHECK(send(fd, "probe", 5, 0) == 5);
	waiting_for_error(fd);
	int value = -1;
	socklen_t size = sizeof(value);
	CHECK(getsockopt(fd, SOL_SOCKET, SO_ERROR, &value, &size) == 0 && value == ECONNREFUSED);
	CHECK(getsockopt(fd, SOL_SOCKET, SO_ERROR, &value, &size) == 0 && value == 0);
	// Without IP_RECVERR, nothing waits on the queue.
	CHECK_ERROR(recv(fd, buffer, sizeof(buffer), MSG_ERRQUEUE), EAGAIN);
	close(fd);

	// Unconnected, it hears nothing without IP_RECVERR.
	fd = socket(AF_INET, SOCK_DGRAM, 0);
	CHECK(fd >= 0);
	CHECK(sendto(fd, "probe", 5, 0, (struct sockaddr *)&there, sizeof(there)) == 5);
	struct pollfd p = {fd, POLLIN, 0};
	CHECK(poll(&p, 1, 300) == 0);
	CHECK_ERROR(recv(fd, buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	// With it, each error waits with the datagram's data, where it went,
	// and who said so.
	value = 1;
	CHECK(setsockopt(fd, SOL_IP, IP_RECVERR, &value, sizeof(value)) == 0);
	value = -1;
	size = sizeof(value);
	CHECK(getsockopt(fd, SOL_IP, IP_RECVERR, &value, &size) == 0 && value == 1);
	CHECK(sendto(fd, "probe", 5, 0, (struct sockaddr *)&there, sizeof(there)) == 5);
	waiting_for_error(fd);
	struct queued error;
	struct sockaddr_in to;
	socklen_t to_len = sizeof(to);
	int flags = 0;
	CHECK(queued_error(fd, buffer, sizeof(buffer), &to, &to_len, SOL_IP, IP_RECVERR, &error, &flags)
			== 5);
	CHECK(!memcmp(buffer, "probe", 5) && (flags & MSG_ERRQUEUE) && !(flags & MSG_TRUNC));
	CHECK(to_len == sizeof(to) && to.sin_family == AF_INET && ntohs(to.sin_port) == closed);
	CHECK(to.sin_addr.s_addr == htonl(INADDR_LOOPBACK));
	CHECK(error.ee.ee_errno == ECONNREFUSED && error.ee.ee_origin == SO_EE_ORIGIN_ICMP);
	CHECK(error.ee.ee_type == 3 && error.ee.ee_code == 3 && error.ee.ee_info == 0);
	struct sockaddr_in *offender = (struct sockaddr_in *)SO_EE_OFFENDER(&error.ee);
	CHECK(offender->sin_family == AF_INET && offender->sin_addr.s_addr == htonl(INADDR_LOOPBACK));
	// Taking it from the queue takes the error a call would fail with.
	CHECK_ERROR(recv(fd, buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	CHECK_ERROR(recv(fd, buffer, sizeof(buffer), MSG_ERRQUEUE), EAGAIN);
	// What does not fit is cut short.
	CHECK(sendto(fd, "probe", 5, 0, (struct sockaddr *)&there, sizeof(there)) == 5);
	waiting_for_error(fd);
	to_len = sizeof(to);
	CHECK(queued_error(fd, buffer, 2, &to, &to_len, SOL_IP, IP_RECVERR, &error, &flags) == 2);
	CHECK(!memcmp(buffer, "pr", 2) && (flags & MSG_TRUNC));
	// Turning IP_RECVERR off throws away what waits, but not the error
	// the next call fails with, as on Linux.
	CHECK(sendto(fd, "probe", 5, 0, (struct sockaddr *)&there, sizeof(there)) == 5);
	waiting_for_error(fd);
	value = 0;
	CHECK(setsockopt(fd, SOL_IP, IP_RECVERR, &value, sizeof(value)) == 0);
	CHECK_ERROR(recv(fd, buffer, sizeof(buffer), MSG_ERRQUEUE), EAGAIN);
	CHECK_ERROR(recv(fd, buffer, sizeof(buffer), MSG_DONTWAIT), ECONNREFUSED);
	CHECK_ERROR(recv(fd, buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	// The type of service is taken and read back, as on Linux, though
	// every packet still goes with 0 (ADR 0345, ADR 0410).
	value = -1;
	size = sizeof(value);
	CHECK(getsockopt(fd, SOL_IP, IP_TOS, &value, &size) == 0 && value == 0);
	value = 0;
	CHECK(setsockopt(fd, SOL_IP, IP_TOS, &value, sizeof(value)) == 0);
	value = 0x10;
	CHECK(setsockopt(fd, SOL_IP, IP_TOS, &value, sizeof(value)) == 0);
	value = -1;
	size = sizeof(value);
	CHECK(getsockopt(fd, SOL_IP, IP_TOS, &value, &size) == 0 && value == 0x10);
	close(fd);

	// IPv6, to a port of ::1's that no socket has.
	int six = socket(AF_INET6, SOCK_DGRAM, 0);
	CHECK(six >= 0);
	value = 1;
	CHECK(setsockopt(six, SOL_IPV6, IPV6_RECVERR, &value, sizeof(value)) == 0);
	struct sockaddr_in6 there6;
	memset(&there6, 0, sizeof(there6));
	there6.sin6_family = AF_INET6;
	there6.sin6_addr = in6addr_loopback;
	there6.sin6_port = htons(closed);
	CHECK(sendto(six, "probe6", 6, 0, (struct sockaddr *)&there6, sizeof(there6)) == 6);
	waiting_for_error(six);
	struct sockaddr_in6 to6;
	socklen_t to6_len = sizeof(to6);
	CHECK(queued_error(six, buffer, sizeof(buffer), &to6, &to6_len, SOL_IPV6, IPV6_RECVERR, &error,
			&flags) == 6);
	CHECK(!memcmp(buffer, "probe6", 6) && to6_len == sizeof(to6));
	CHECK(to6.sin6_family == AF_INET6 && IN6_IS_ADDR_LOOPBACK(&to6.sin6_addr));
	CHECK(ntohs(to6.sin6_port) == closed);
	CHECK(error.ee.ee_errno == ECONNREFUSED && error.ee.ee_origin == SO_EE_ORIGIN_ICMP6);
	CHECK(error.ee.ee_type == 1 && error.ee.ee_code == 4);
	struct sockaddr_in6 *offender6 = (struct sockaddr_in6 *)SO_EE_OFFENDER(&error.ee);
	CHECK(offender6->sin6_family == AF_INET6 && IN6_IS_ADDR_LOOPBACK(&offender6->sin6_addr));
	value = -1;
	size = sizeof(value);
	CHECK(getsockopt(six, SOL_IPV6, IPV6_TCLASS, &value, &size) == 0 && value == 0);
	value = 0x20;
	CHECK(setsockopt(six, SOL_IPV6, IPV6_TCLASS, &value, sizeof(value)) == 0);
	value = -1;
	size = sizeof(value);
	CHECK(getsockopt(six, SOL_IPV6, IPV6_TCLASS, &value, &size) == 0 && value == 0x20);
	close(six);
}

// lo's name and number, as /kernel/class/net gives them, and a socket
// bound to it with SO_BINDTODEVICE, which still reaches 127.0.0.1.
// AF_INET6 over ::1 (ADR 0332).
static void ipv6(void) {
	int server = socket(AF_INET6, SOCK_STREAM, 0);
	CHECK(server >= 0);
	struct sockaddr_in6 at;
	memset(&at, 0, sizeof(at));
	at.sin6_family = AF_INET6;
	at.sin6_addr = in6addr_loopback;
	CHECK(bind(server, (struct sockaddr *)&at, sizeof(at)) == 0);
	CHECK(listen(server, 1) == 0);
	socklen_t len = sizeof(at);
	CHECK(getsockname(server, (struct sockaddr *)&at, &len) == 0);
	CHECK(at.sin6_family == AF_INET6 && at.sin6_port != 0);
	int client = socket(AF_INET6, SOCK_STREAM, 0);
	CHECK(client >= 0);
	CHECK(connect(client, (struct sockaddr *)&at, sizeof(at)) == 0);
	struct sockaddr_in6 peer;
	len = sizeof(peer);
	int accepted = accept(server, (struct sockaddr *)&peer, &len);
	CHECK(accepted >= 0);
	CHECK(peer.sin6_family == AF_INET6 && IN6_IS_ADDR_LOOPBACK(&peer.sin6_addr));
	CHECK(write(client, "six", 3) == 3);
	char buffer[8];
	CHECK(read(accepted, buffer, sizeof(buffer)) == 3 && !memcmp(buffer, "six", 3));
	close(accepted);
	close(client);
	close(server);
	// A socket bound to :: takes IPv4 too, unless IPV6_V6ONLY.
	int any = socket(AF_INET6, SOCK_STREAM, 0);
	CHECK(any >= 0);
	int only = 1;
	len = sizeof(only);
	CHECK(getsockopt(any, IPPROTO_IPV6, IPV6_V6ONLY, &only, &len) == 0 && only == 0);
	memset(&at, 0, sizeof(at));
	at.sin6_family = AF_INET6;
	at.sin6_addr = in6addr_any;
	CHECK(bind(any, (struct sockaddr *)&at, sizeof(at)) == 0);
	CHECK(listen(any, 1) == 0);
	len = sizeof(at);
	CHECK(getsockname(any, (struct sockaddr *)&at, &len) == 0);
	int four = socket(AF_INET, SOCK_STREAM, 0);
	struct sockaddr_in to = loopback(ntohs(at.sin6_port));
	CHECK(connect(four, (struct sockaddr *)&to, sizeof(to)) == 0);
	len = sizeof(peer);
	accepted = accept(any, (struct sockaddr *)&peer, &len);
	CHECK(accepted >= 0);
	CHECK(peer.sin6_family == AF_INET6 && IN6_IS_ADDR_V4MAPPED(&peer.sin6_addr));
	close(accepted);
	close(four);
	close(any);
}

static void interfaces(void) {
	CHECK(if_nametoindex("lo") == 1);
	CHECK(if_nametoindex("net0") == 0 && errno == ENODEV);
	CHECK(if_nametoindex("../lo") == 0);
	char name[IF_NAMESIZE];
	CHECK(if_indextoname(1, name) == name && !strcmp(name, "lo"));
	CHECK(if_indextoname(2, name) == NULL && errno == ENXIO);
	struct if_nameindex *list = if_nameindex();
	CHECK(list != NULL);
	if(list) {
		CHECK(list[0].if_index == 1 && !strcmp(list[0].if_name, "lo"));
		CHECK(list[1].if_index == 0 && list[1].if_name == NULL);
		if_freenameindex(list);
	}

	int server = socket(AF_INET, SOCK_STREAM, 0);
	struct sockaddr_in at = loopback(0);
	CHECK(bind(server, (struct sockaddr *)&at, sizeof(at)) == 0);
	CHECK(listen(server, 1) == 0);
	int fd = socket(AF_INET, SOCK_STREAM, 0);
	char bound[IF_NAMESIZE] = "x";
	socklen_t len = sizeof(bound);
	CHECK(getsockopt(fd, SOL_SOCKET, SO_BINDTODEVICE, bound, &len) == 0 && len == 0);
	// A name without its NUL, as the length says.
	CHECK(setsockopt(fd, SOL_SOCKET, SO_BINDTODEVICE, "lox", 2) == 0);
	len = sizeof(bound);
	CHECK(getsockopt(fd, SOL_SOCKET, SO_BINDTODEVICE, bound, &len) == 0);
	CHECK(len == 3 && !strcmp(bound, "lo"));
	len = 4;
	CHECK_ERROR(getsockopt(fd, SOL_SOCKET, SO_BINDTODEVICE, bound, &len), EINVAL);
	CHECK_ERROR(setsockopt(fd, SOL_SOCKET, SO_BINDTODEVICE, "nosuch", 6), ENODEV);
	at = loopback(port_of(server));
	CHECK(connect(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	int taken = accept(server, NULL, NULL);
	CHECK(taken >= 0);
	// A stream bound once it connected stays as it is.
	CHECK_ERROR(setsockopt(fd, SOL_SOCKET, SO_BINDTODEVICE, "", 0), EINVAL);
	close(taken);
	close(fd);
	close(server);

	// A datagram socket bound to lo, and then to none.
	int d = socket(AF_INET, SOCK_DGRAM, 0);
	CHECK(setsockopt(d, SOL_SOCKET, SO_BINDTODEVICE, "lo", 3) == 0);
	at = loopback(0);
	CHECK(bind(d, (struct sockaddr *)&at, sizeof(at)) == 0);
	struct sockaddr_in self = loopback(port_of(d));
	CHECK(sendto(d, "x", 1, 0, (struct sockaddr *)&self, sizeof(self)) == 1);
	char byte = 0;
	CHECK(recv(d, &byte, 1, 0) == 1 && byte == 'x');
	CHECK(setsockopt(d, SOL_SOCKET, SO_BINDTODEVICE, "", 0) == 0);
	len = sizeof(bound);
	CHECK(getsockopt(d, SOL_SOCKET, SO_BINDTODEVICE, bound, &len) == 0 && len == 0);
	close(d);
}

// A ping socket (ADR 0336), which any program may make: an echo request
// to 127.0.0.1 and one to ::1, whose identifier the kernel makes the
// socket's, as its port; the replies, without their IP headers; a
// connected one; and what a ping socket does not send.
static void ping_sockets(void) {
	int fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_ICMP);
	CHECK(fd >= 0);
	int value = 0;
	socklen_t size = sizeof(value);
	CHECK(getsockopt(fd, SOL_SOCKET, SO_TYPE, &value, &size) == 0 && value == SOCK_DGRAM);
	CHECK(getsockopt(fd, SOL_SOCKET, SO_PROTOCOL, &value, &size) == 0 && value == IPPROTO_ICMP);
	struct timeval wait = {5, 0};
	CHECK(setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &wait, sizeof(wait)) == 0);
	unsigned char request[16] = {8, 0, 0, 0, 0x12, 0x34, 0, 7, 'e', 'c', 'h', 'o', ' ', 'm', 'e', '!'};
	struct sockaddr_in to = loopback(0);
	CHECK(sendto(fd, request, sizeof(request), 0, (struct sockaddr *)&to, sizeof(to))
			== (ssize_t)sizeof(request));
	unsigned short ident = port_of(fd);
	CHECK(ident != 0);
	unsigned char reply[64];
	struct sockaddr_in from;
	socklen_t from_len = sizeof(from);
	CHECK(recvfrom(fd, reply, sizeof(reply), 0, (struct sockaddr *)&from, &from_len)
			== (ssize_t)sizeof(request));
	CHECK(reply[0] == 0 && reply[1] == 0 && ((reply[4] << 8) | reply[5]) == ident);
	CHECK(reply[6] == 0 && reply[7] == 7 && !memcmp(reply + 8, request + 8, 8));
	CHECK(from.sin_family == AF_INET && from.sin_addr.s_addr == htonl(INADDR_LOOPBACK));
	// Only echo requests go, with their header whole.
	unsigned char timestamp[20] = {13};
	CHECK_ERROR(sendto(fd, timestamp, sizeof(timestamp), 0, (struct sockaddr *)&to, sizeof(to)),
			EINVAL);
	CHECK_ERROR(sendto(fd, request, 4, 0, (struct sockaddr *)&to, sizeof(to)), EINVAL);
	// Connected, it takes replies from there alone.
	CHECK(connect(fd, (struct sockaddr *)&to, sizeof(to)) == 0);
	request[7] = 8;
	CHECK(send(fd, request, sizeof(request), 0) == (ssize_t)sizeof(request));
	CHECK(recv(fd, reply, sizeof(reply), 0) == (ssize_t)sizeof(request) && reply[7] == 8);
	// A ping socket has no groups.
	struct ip_mreqn group = {.imr_multiaddr.s_addr = inet_addr("239.1.2.3"), .imr_ifindex = 1};
	CHECK_ERROR(setsockopt(fd, IPPROTO_IP, IP_ADD_MEMBERSHIP, &group, sizeof(group)), EPROTO);
	close(fd);

	int six = socket(AF_INET6, SOCK_DGRAM, IPPROTO_ICMPV6);
	CHECK(six >= 0);
	size = sizeof(value);
	CHECK(getsockopt(six, SOL_SOCKET, SO_PROTOCOL, &value, &size) == 0 && value == IPPROTO_ICMPV6);
	CHECK(setsockopt(six, SOL_SOCKET, SO_RCVTIMEO, &wait, sizeof(wait)) == 0);
	request[0] = 128;
	struct sockaddr_in6 to6;
	memset(&to6, 0, sizeof(to6));
	to6.sin6_family = AF_INET6;
	to6.sin6_addr = in6addr_loopback;
	CHECK(sendto(six, request, sizeof(request), 0, (struct sockaddr *)&to6, sizeof(to6))
			== (ssize_t)sizeof(request));
	struct sockaddr_in6 from6;
	socklen_t from6_len = sizeof(from6);
	CHECK(recvfrom(six, reply, sizeof(reply), 0, (struct sockaddr *)&from6, &from6_len)
			== (ssize_t)sizeof(request));
	CHECK(reply[0] == 129 && reply[7] == 8 && !memcmp(reply + 8, request + 8, 8));
	CHECK(from6.sin6_family == AF_INET6 && IN6_IS_ADDR_LOOPBACK(&from6.sin6_addr));
	close(six);
	CHECK_ERROR(socket(AF_INET, SOCK_DGRAM, IPPROTO_ICMPV6), EPROTONOSUPPORT);
	CHECK_ERROR(socket(AF_INET6, SOCK_DGRAM, IPPROTO_ICMP), EPROTONOSUPPORT);
}

// IP_PKTINFO and IPV6_RECVPKTINFO (ADR 0336): the interface each
// datagram came by and the address it came to, as mDNS asks; MSG_CTRUNC
// without room for them; and nothing when they are not asked for.
static void packet_info(void) {
	int fd = socket(AF_INET, SOCK_DGRAM, 0);
	struct sockaddr_in at = loopback(0);
	CHECK(bind(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	int on = 1, value = 0;
	socklen_t size = sizeof(value);
	CHECK(setsockopt(fd, IPPROTO_IP, IP_PKTINFO, &on, sizeof(on)) == 0);
	CHECK(getsockopt(fd, IPPROTO_IP, IP_PKTINFO, &value, &size) == 0 && value == 1);
	struct sockaddr_in self = loopback(port_of(fd));
	char data[8];
	struct iovec iov = {data, sizeof(data)};
	union {
		struct cmsghdr align;
		char buffer[CMSG_SPACE(sizeof(struct in6_pktinfo))];
	} control;
	struct msghdr hdr;
	memset(&hdr, 0, sizeof(hdr));
	hdr.msg_iov = &iov;
	hdr.msg_iovlen = 1;
	hdr.msg_control = control.buffer;
	hdr.msg_controllen = sizeof(control.buffer);
	CHECK(sendto(fd, "info", 4, 0, (struct sockaddr *)&self, sizeof(self)) == 4);
	CHECK(recvmsg(fd, &hdr, 0) == 4 && hdr.msg_flags == 0);
	struct cmsghdr *c = CMSG_FIRSTHDR(&hdr);
	CHECK(c != NULL && c->cmsg_level == IPPROTO_IP && c->cmsg_type == IP_PKTINFO);
	if(c) {
		struct in_pktinfo info;
		memcpy(&info, CMSG_DATA(c), sizeof(info));
		CHECK(info.ipi_ifindex == 1);
		CHECK(info.ipi_addr.s_addr == htonl(INADDR_LOOPBACK));
		CHECK(info.ipi_spec_dst.s_addr == htonl(INADDR_LOOPBACK));
		CHECK(CMSG_NXTHDR(&hdr, c) == NULL);
	}
	CHECK(sendto(fd, "info", 4, 0, (struct sockaddr *)&self, sizeof(self)) == 4);
	hdr.msg_controllen = sizeof(struct cmsghdr) - 1;
	CHECK(recvmsg(fd, &hdr, 0) == 4 && (hdr.msg_flags & MSG_CTRUNC) && hdr.msg_controllen == 0);
	on = 0;
	CHECK(setsockopt(fd, IPPROTO_IP, IP_PKTINFO, &on, sizeof(on)) == 0);
	CHECK(sendto(fd, "info", 4, 0, (struct sockaddr *)&self, sizeof(self)) == 4);
	hdr.msg_controllen = sizeof(control.buffer);
	CHECK(recvmsg(fd, &hdr, 0) == 4 && hdr.msg_flags == 0 && hdr.msg_controllen == 0);
	close(fd);

	int six = socket(AF_INET6, SOCK_DGRAM, 0);
	struct sockaddr_in6 at6;
	memset(&at6, 0, sizeof(at6));
	at6.sin6_family = AF_INET6;
	at6.sin6_addr = in6addr_loopback;
	CHECK(bind(six, (struct sockaddr *)&at6, sizeof(at6)) == 0);
	on = 1;
	CHECK(setsockopt(six, IPPROTO_IPV6, IPV6_RECVPKTINFO, &on, sizeof(on)) == 0);
	socklen_t len = sizeof(at6);
	CHECK(getsockname(six, (struct sockaddr *)&at6, &len) == 0);
	CHECK(sendto(six, "six", 3, 0, (struct sockaddr *)&at6, sizeof(at6)) == 3);
	hdr.msg_controllen = sizeof(control.buffer);
	CHECK(recvmsg(six, &hdr, 0) == 3);
	c = CMSG_FIRSTHDR(&hdr);
	CHECK(c != NULL && c->cmsg_level == IPPROTO_IPV6 && c->cmsg_type == IPV6_PKTINFO);
	if(c) {
		struct in6_pktinfo info;
		memcpy(&info, CMSG_DATA(c), sizeof(info));
		CHECK(info.ipi6_ifindex == 1 && IN6_IS_ADDR_LOOPBACK(&info.ipi6_addr));
	}
	close(six);
}

// SO_REUSEPORT and SO_REUSEADDR (ADR 0336), as on Linux: datagram
// sockets that set the same one share a port, an IPv6 one on :: with
// IPv4 ones on 0.0.0.0 as mDNS's do, and a datagram to it reaches one of
// them; a socket that sets only the other one is refused it.
static void reuse_port(void) {
	int a = socket(AF_INET, SOCK_DGRAM, 0);
	int b = socket(AF_INET, SOCK_DGRAM, 0);
	int c = socket(AF_INET, SOCK_DGRAM, 0);
	int six = socket(AF_INET6, SOCK_DGRAM, 0);
	int on = 1, value = 0;
	socklen_t size = sizeof(value);
	CHECK(getsockopt(a, SOL_SOCKET, SO_REUSEPORT, &value, &size) == 0 && value == 0);
	CHECK(setsockopt(a, SOL_SOCKET, SO_REUSEPORT, &on, sizeof(on)) == 0);
	CHECK(getsockopt(a, SOL_SOCKET, SO_REUSEPORT, &value, &size) == 0 && value == 1);
	CHECK(setsockopt(b, SOL_SOCKET, SO_REUSEPORT, &on, sizeof(on)) == 0);
	CHECK(setsockopt(c, SOL_SOCKET, SO_REUSEADDR, &on, sizeof(on)) == 0);
	CHECK(setsockopt(six, SOL_SOCKET, SO_REUSEADDR, &on, sizeof(on)) == 0);
	CHECK(setsockopt(six, SOL_SOCKET, SO_REUSEPORT, &on, sizeof(on)) == 0);
	struct sockaddr_in any = loopback(0);
	any.sin_addr.s_addr = htonl(INADDR_ANY);
	CHECK(bind(a, (struct sockaddr *)&any, sizeof(any)) == 0);
	unsigned short port = port_of(a);
	any.sin_port = htons(port);
	CHECK(bind(b, (struct sockaddr *)&any, sizeof(any)) == 0);
	CHECK_ERROR(bind(c, (struct sockaddr *)&any, sizeof(any)), EADDRINUSE);
	struct sockaddr_in6 any6;
	memset(&any6, 0, sizeof(any6));
	any6.sin6_family = AF_INET6;
	any6.sin6_addr = in6addr_any;
	any6.sin6_port = htons(port);
	CHECK(bind(six, (struct sockaddr *)&any6, sizeof(any6)) == 0);
	struct sockaddr_in to = loopback(port);
	CHECK(sendto(c, "one", 3, 0, (struct sockaddr *)&to, sizeof(to)) == 3);
	struct pollfd p[3] = {{a, POLLIN, 0}, {b, POLLIN, 0}, {six, POLLIN, 0}};
	CHECK(poll(p, 3, 5000) >= 1);
	close(a);
	close(b);
	close(c);
	close(six);
}

// Multicast (ADR 0336), on lo, the only interface this boot has: the
// options and their defaults, as ints and as bytes; groups joined and
// left on an interface named by number or address; a datagram to a
// group, which reaches the sockets on its port that joined it and no
// other; and the errors.
static void multicast(void) {
	int fd = socket(AF_INET, SOCK_DGRAM, 0);
	int value = -1;
	socklen_t size = sizeof(value);
	CHECK(getsockopt(fd, IPPROTO_IP, IP_MULTICAST_TTL, &value, &size) == 0 && value == 1);
	size = sizeof(value);
	CHECK(getsockopt(fd, IPPROTO_IP, IP_MULTICAST_LOOP, &value, &size) == 0 && value == 1);
	int ttl = 255;
	CHECK(setsockopt(fd, IPPROTO_IP, IP_MULTICAST_TTL, &ttl, sizeof(ttl)) == 0);
	size = sizeof(value);
	CHECK(getsockopt(fd, IPPROTO_IP, IP_MULTICAST_TTL, &value, &size) == 0 && value == 255);
	unsigned char byte = 4;
	CHECK(setsockopt(fd, IPPROTO_IP, IP_MULTICAST_TTL, &byte, 1) == 0);
	byte = 0;
	size = 1;
	CHECK(getsockopt(fd, IPPROTO_IP, IP_MULTICAST_TTL, &byte, &size) == 0 && byte == 4 && size == 1);
	ttl = 256;
	CHECK_ERROR(setsockopt(fd, IPPROTO_IP, IP_MULTICAST_TTL, &ttl, sizeof(ttl)), EINVAL);
	ttl = -1;
	CHECK(setsockopt(fd, IPPROTO_IP, IP_MULTICAST_TTL, &ttl, sizeof(ttl)) == 0);
	size = sizeof(value);
	CHECK(getsockopt(fd, IPPROTO_IP, IP_MULTICAST_TTL, &value, &size) == 0 && value == 1);

	// Joined on lo by its number, and by its address; each once.
	struct ip_mreqn by_number = {.imr_multiaddr.s_addr = inet_addr("239.1.2.3"), .imr_ifindex = 1};
	CHECK(setsockopt(fd, IPPROTO_IP, IP_ADD_MEMBERSHIP, &by_number, sizeof(by_number)) == 0);
	CHECK_ERROR(setsockopt(fd, IPPROTO_IP, IP_ADD_MEMBERSHIP, &by_number, sizeof(by_number)),
			EADDRINUSE);
	struct ip_mreq by_address = {.imr_multiaddr.s_addr = inet_addr("239.1.2.4"),
			.imr_interface.s_addr = htonl(INADDR_LOOPBACK)};
	CHECK(setsockopt(fd, IPPROTO_IP, IP_ADD_MEMBERSHIP, &by_address, sizeof(by_address)) == 0);
	CHECK(setsockopt(fd, IPPROTO_IP, IP_DROP_MEMBERSHIP, &by_address, sizeof(by_address)) == 0);
	CHECK_ERROR(setsockopt(fd, IPPROTO_IP, IP_DROP_MEMBERSHIP, &by_address, sizeof(by_address)),
			EADDRNOTAVAIL);
	// Not a group; an interface not there; and with no card, no route to
	// a group, so that the interface must be named.
	struct ip_mreq wrong = {.imr_multiaddr.s_addr = htonl(INADDR_LOOPBACK),
			.imr_interface.s_addr = htonl(INADDR_LOOPBACK)};
	CHECK_ERROR(setsockopt(fd, IPPROTO_IP, IP_ADD_MEMBERSHIP, &wrong, sizeof(wrong)), EINVAL);
	struct ip_mreqn nowhere = {.imr_multiaddr.s_addr = inet_addr("239.1.2.5"), .imr_ifindex = 9};
	CHECK_ERROR(setsockopt(fd, IPPROTO_IP, IP_ADD_MEMBERSHIP, &nowhere, sizeof(nowhere)), ENODEV);
	struct ip_mreq unnamed = {.imr_multiaddr.s_addr = inet_addr("239.1.2.5"),
			.imr_interface.s_addr = htonl(INADDR_ANY)};
	CHECK_ERROR(setsockopt(fd, IPPROTO_IP, IP_ADD_MEMBERSHIP, &unnamed, sizeof(unnamed)), ENODEV);
	struct ip_mreq away = {.imr_multiaddr.s_addr = inet_addr("239.1.2.5"),
			.imr_interface.s_addr = inet_addr("10.9.9.9")};
	CHECK_ERROR(setsockopt(fd, IPPROTO_IP, IP_ADD_MEMBERSHIP, &away, sizeof(away)), ENODEV);
	CHECK_ERROR(setsockopt(fd, IPPROTO_IP, IP_ADD_MEMBERSHIP, &by_number, 4), EINVAL);

	// Bound to the group's port: the socket that joined takes what is sent
	// to the group, and one that did not does not.
	int other = socket(AF_INET, SOCK_DGRAM, 0);
	int on = 1;
	CHECK(setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &on, sizeof(on)) == 0);
	CHECK(setsockopt(other, SOL_SOCKET, SO_REUSEADDR, &on, sizeof(on)) == 0);
	struct sockaddr_in any = loopback(0);
	any.sin_addr.s_addr = htonl(INADDR_ANY);
	CHECK(bind(fd, (struct sockaddr *)&any, sizeof(any)) == 0);
	any.sin_port = htons(port_of(fd));
	CHECK(bind(other, (struct sockaddr *)&any, sizeof(any)) == 0);
	int sender = socket(AF_INET, SOCK_DGRAM, 0);
	struct in_addr lo = {htonl(INADDR_LOOPBACK)};
	CHECK(setsockopt(sender, IPPROTO_IP, IP_MULTICAST_IF, &lo, sizeof(lo)) == 0);
	struct in_addr chosen = {0};
	size = sizeof(chosen);
	CHECK(getsockopt(sender, IPPROTO_IP, IP_MULTICAST_IF, &chosen, &size) == 0);
	CHECK(chosen.s_addr == htonl(INADDR_LOOPBACK));
	struct sockaddr_in group = any;
	group.sin_addr.s_addr = inet_addr("239.1.2.3");
	CHECK(sendto(sender, "all", 3, 0, (struct sockaddr *)&group, sizeof(group)) == 3);
	struct pollfd p = {fd, POLLIN, 0};
	CHECK(poll(&p, 1, 5000) == 1);
	char buffer[8];
	CHECK(recv(fd, buffer, sizeof(buffer), MSG_DONTWAIT) == 3 && !memcmp(buffer, "all", 3));
	CHECK_ERROR(recv(other, buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	// Left, it takes no more.
	CHECK(setsockopt(fd, IPPROTO_IP, IP_DROP_MEMBERSHIP, &by_number, sizeof(by_number)) == 0);
	CHECK(sendto(sender, "gone", 4, 0, (struct sockaddr *)&group, sizeof(group)) == 4);
	CHECK(poll(&p, 1, 200) == 0);
	close(sender);
	close(other);
	close(fd);

	int six = socket(AF_INET6, SOCK_DGRAM, 0);
	size = sizeof(value);
	CHECK(getsockopt(six, IPPROTO_IPV6, IPV6_MULTICAST_HOPS, &value, &size) == 0 && value == 1);
	int hops = -1;
	CHECK(setsockopt(six, IPPROTO_IPV6, IPV6_MULTICAST_HOPS, &hops, sizeof(hops)) == 0);
	int loop = 2;
	CHECK_ERROR(setsockopt(six, IPPROTO_IPV6, IPV6_MULTICAST_LOOP, &loop, sizeof(loop)), EINVAL);
	struct ipv6_mreq mdns6;
	memset(&mdns6, 0, sizeof(mdns6));
	inet_pton(AF_INET6, "ff02::fb", &mdns6.ipv6mr_multiaddr);
	mdns6.ipv6mr_interface = 1;
	CHECK(setsockopt(six, IPPROTO_IPV6, IPV6_ADD_MEMBERSHIP, &mdns6, sizeof(mdns6)) == 0);
	CHECK(setsockopt(six, IPPROTO_IPV6, IPV6_DROP_MEMBERSHIP, &mdns6, sizeof(mdns6)) == 0);
	unsigned int index = 1;
	CHECK(setsockopt(six, IPPROTO_IPV6, IPV6_MULTICAST_IF, &index, sizeof(index)) == 0);
	close(six);
	// A stream socket has no groups, as on Linux.
	int stream = socket(AF_INET, SOCK_STREAM, 0);
	CHECK_ERROR(setsockopt(stream, IPPROTO_IP, IP_ADD_MEMBERSHIP, &by_number, sizeof(by_number)),
			EPROTO);
	close(stream);
}

// getifaddrs (ADR 0336): lo's IPv4 and IPv6 addresses, their masks and
// flags, as /kernel/class/net gives them.
static void addresses(void) {
	struct ifaddrs *list = NULL;
	CHECK(getifaddrs(&list) == 0 && list != NULL);
	int four = 0, six = 0;
	for(struct ifaddrs *i = list; i; i = i->ifa_next) {
		CHECK(!strcmp(i->ifa_name, "lo"));
		CHECK((i->ifa_flags & (IFF_UP | IFF_LOOPBACK | IFF_RUNNING)) == (IFF_UP | IFF_LOOPBACK | IFF_RUNNING));
		CHECK(!(i->ifa_flags & IFF_BROADCAST) && i->ifa_broadaddr == NULL);
		if(!i->ifa_addr)
			continue;
		if(i->ifa_addr->sa_family == AF_INET) {
			four++;
			struct sockaddr_in *in = (struct sockaddr_in *)i->ifa_addr;
			struct sockaddr_in *mask = (struct sockaddr_in *)i->ifa_netmask;
			CHECK(in->sin_addr.s_addr == htonl(INADDR_LOOPBACK));
			CHECK(mask && mask->sin_addr.s_addr == htonl(0xff000000));
		} else if(i->ifa_addr->sa_family == AF_INET6) {
			six++;
			struct sockaddr_in6 *in6 = (struct sockaddr_in6 *)i->ifa_addr;
			struct sockaddr_in6 *mask = (struct sockaddr_in6 *)i->ifa_netmask;
			CHECK(IN6_IS_ADDR_LOOPBACK(&in6->sin6_addr) && in6->sin6_scope_id == 0);
			static const unsigned char all[16] = {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
					0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff};
			CHECK(mask && !memcmp(&mask->sin6_addr, all, 16));
		}
	}
	CHECK(four == 1 && six == 1);
	freeifaddrs(list);
}

// Deletes every network capability the process has, but the one in slot
// `keep` (0 for none).
static void drop_network(uint32_t keep) {
	for(uint32_t i = 0; i < RADIX_STARTUP_NETWORK_SLOTS; i++) {
		uint32_t slot = RADIX_STARTUP_SLOT_NETWORK + i;
		if(slot != keep)
			radix_capability_delete(slot);
	}
}

// Derives a network capability for `count` ports from `first` with
// `rights`, from the one in SLOT_NETWORK, into the last network slot,
// and keeps only that one.
static void only(uint32_t first, uint32_t count, uint64_t rights) {
	uint32_t slot = RADIX_STARTUP_SLOT_NETWORK + RADIX_STARTUP_NETWORK_SLOTS - 1;
	radix_capability_delete(slot);
	uint64_t kept = RADIX_RIGHTS_COPY | RADIX_RIGHTS_TRANSFER;
	CHECK(radix_network_restrict(RADIX_STARTUP_SLOT_NETWORK, slot, first, count, rights | kept)
			== 0);
	drop_network(slot);
}

static void capabilities(void) {
	// The runner was given the network, and so was this test.
	struct radix_network_ports_results ports;
	CHECK(radix_network_ports(RADIX_STARTUP_SLOT_NETWORK, &ports) == 0);
	CHECK(ports.first == 0 && ports.count == 65536);
	int server = listener();
	unsigned short port = port_of(server);
	// Without a network capability no socket can be made.
	pid_t pid = check_fork();
	if(pid == 0) {
		drop_network(0);
		CHECK_ERROR(socket(AF_INET, SOCK_STREAM, 0), EACCES);
		CHECK_ERROR(socket(AF_INET, SOCK_DGRAM, 0), EACCES);
		// Unix sockets need none.
		int fds[2];
		CHECK(socketpair(AF_UNIX, SOCK_STREAM, 0, fds) == 0);
		check_exit_child();
	}
	CHECK_CHILD(pid);
	// Allowed only to connect out: it connects, but cannot bind a port
	// of its own or listen.
	pid = check_fork();
	if(pid == 0) {
		only(0, 65536, RADIX_RIGHTS_SEND);
		int client = connected(port);
		CHECK(write(client, "out", 3) == 3);
		close(client);
		int fd = socket(AF_INET, SOCK_STREAM, 0);
		struct sockaddr_in at = loopback(9000);
		CHECK_ERROR(bind(fd, (struct sockaddr *)&at, sizeof(at)), EACCES);
		CHECK_ERROR(listen(fd, 1), EACCES);
		close(fd);
		// The send right reaches past the networks this machine is on,
		// not them: no group to join or send to (ADR 0337).
		int d = socket(AF_INET, SOCK_DGRAM, 0);
		struct ip_mreqn group = {.imr_multiaddr.s_addr = inet_addr("239.1.2.3"), .imr_ifindex = 1};
		CHECK_ERROR(setsockopt(d, IPPROTO_IP, IP_ADD_MEMBERSHIP, &group, sizeof(group)), EACCES);
		struct sockaddr_in to_group = loopback(9);
		to_group.sin_addr.s_addr = group.imr_multiaddr.s_addr;
		CHECK_ERROR(sendto(d, "x", 1, 0, (struct sockaddr *)&to_group, sizeof(to_group)), EACCES);
		close(d);
		check_exit_child();
	}
	int accepted = accept(server, NULL, NULL);
	CHECK(accepted >= 0);
	char buffer[8];
	CHECK(read(accepted, buffer, sizeof(buffer)) == 3);
	close(accepted);
	CHECK_CHILD(pid);
	// The local right alone: this machine, and the groups of the networks
	// it is on.
	pid = check_fork();
	if(pid == 0) {
		only(0, 65536, RADIX_RIGHTS_LOCAL);
		int client = connected(port);
		CHECK(write(client, "near", 4) == 4);
		close(client);
		int d = socket(AF_INET, SOCK_DGRAM, 0);
		struct ip_mreqn group = {.imr_multiaddr.s_addr = inet_addr("239.1.2.3"), .imr_ifindex = 1};
		CHECK(setsockopt(d, IPPROTO_IP, IP_ADD_MEMBERSHIP, &group, sizeof(group)) == 0);
		close(d);
		check_exit_child();
	}
	accepted = accept(server, NULL, NULL);
	CHECK(accepted >= 0);
	CHECK(read(accepted, buffer, sizeof(buffer)) == 4);
	close(accepted);
	CHECK_CHILD(pid);
	// Allowed only to listen on ports 9000 to 9009: it binds and listens
	// there, but cannot connect out, bind another port or send a datagram.
	pid = check_fork();
	if(pid == 0) {
		only(9000, 10, RADIX_RIGHTS_RECEIVE);
		int fd = socket(AF_INET, SOCK_STREAM, 0);
		struct sockaddr_in at = loopback(9005);
		CHECK(bind(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
		CHECK(listen(fd, 1) == 0);
		close(fd);
		int other = socket(AF_INET, SOCK_STREAM, 0);
		at = loopback(9010);
		CHECK_ERROR(bind(other, (struct sockaddr *)&at, sizeof(at)), EACCES);
		struct sockaddr_in to = loopback(port);
		CHECK_ERROR(connect(other, (struct sockaddr *)&to, sizeof(to)), EACCES);
		close(other);
		int d = socket(AF_INET, SOCK_DGRAM, 0);
		CHECK_ERROR(sendto(d, "x", 1, 0, (struct sockaddr *)&to, sizeof(to)), EACCES);
		close(d);
		check_exit_child();
	}
	CHECK_CHILD(pid);
	close(server);
}

int main(void) {
	check_start("inet");
	RUN(bench_order);
	RUN(short_connections);
	RUN(many_connections);
	RUN(accept_while_another_opens);
	RUN(tcp);
	RUN(non_blocking);
	RUN(unbound);
	RUN(shared);
	RUN(udp);
	RUN(peeking);
	RUN(numeric_names);
	RUN(errors);
	RUN(icmp_errors);
	RUN(ipv6);
	RUN(interfaces);
	RUN(ping_sockets);
	RUN(packet_info);
	RUN(reuse_port);
	RUN(multicast);
	RUN(addresses);
	RUN(capabilities);
	return check_end();
}
