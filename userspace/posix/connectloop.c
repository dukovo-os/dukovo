// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the sockets boot tests run beside a restart (ADR 0477): a client
// that makes connections to a TCP host and port one after another, sends
// a request if it is given one, reads each answer to its end, and says
// how it went:
//
//     connections N refused R failed F
//
// A connection is refused when connecting fails with ECONNREFUSED, which
// is what a restart that does not keep its sockets causes, and failed
// for any other reason: another error, no answer within 20 seconds, or
// an answer of no bytes. Usage: connectloop HOST PORT COUNT [PAUSE_MS
// [REQUEST]], where REQUEST may hold \r and \n for a carriage return and
// a line feed (a web server answers nothing until it has a request).

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

// How long each step of one connection may take, in milliseconds.
#define STEP_MS 20000

enum outcome { ANSWERED, REFUSED, FAILED };

// `text` with each `\r` and `\n` of two characters made one, written
// into `out`, which is as large as `text`.
static size_t unescape(const char *text, char *out) {
	size_t size = 0;
	for(; *text; text++) {
		if(text[0] == '\\' && text[1] == 'r') {
			out[size++] = '\r';
			text++;
		} else if(text[0] == '\\' && text[1] == 'n') {
			out[size++] = '\n';
			text++;
		} else {
			out[size++] = *text;
		}
	}
	return size;
}

// Fills `address` from `host` and `port`, an IPv4 or an IPv6 address;
// returns its length, or 0 when `host` is neither.
static socklen_t make_address(const char *host, int port, struct sockaddr_storage *address) {
	memset(address, 0, sizeof(*address));
	struct sockaddr_in *v4 = (struct sockaddr_in *)address;
	if(inet_pton(AF_INET, host, &v4->sin_addr) == 1) {
		v4->sin_family = AF_INET;
		v4->sin_port = htons((unsigned short)port);
		return sizeof(*v4);
	}
	struct sockaddr_in6 *v6 = (struct sockaddr_in6 *)address;
	if(inet_pton(AF_INET6, host, &v6->sin6_addr) == 1) {
		v6->sin6_family = AF_INET6;
		v6->sin6_port = htons((unsigned short)port);
		return sizeof(*v6);
	}
	return 0;
}

// Makes one connection to `address`, sends `request` if it has bytes,
// and reads the answer to its end.
static enum outcome connect_once(const struct sockaddr_storage *address, socklen_t length,
		const char *request, size_t request_size) {
	int fd = socket(address->ss_family, SOCK_STREAM, 0);
	if(fd < 0)
		return FAILED;
	fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK);
	enum outcome outcome = FAILED;
	if(connect(fd, (const struct sockaddr *)address, length) < 0) {
		if(errno == ECONNREFUSED) {
			outcome = REFUSED;
			goto done;
		}
		if(errno != EINPROGRESS)
			goto done;
		struct pollfd wait = {.fd = fd, .events = POLLOUT};
		if(poll(&wait, 1, STEP_MS) != 1)
			goto done;
		int error = 0;
		socklen_t size = sizeof(error);
		getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &size);
		if(error == ECONNREFUSED)
			outcome = REFUSED;
		if(error)
			goto done;
	}
	if(request_size && write(fd, request, request_size) != (ssize_t)request_size)
		goto done;
	size_t total = 0;
	for(;;) {
		struct pollfd wait = {.fd = fd, .events = POLLIN};
		if(poll(&wait, 1, STEP_MS) != 1)
			goto done;
		char buffer[1024];
		ssize_t got = read(fd, buffer, sizeof(buffer));
		if(got < 0 && errno == EAGAIN)
			continue;
		if(got < 0)
			goto done;
		if(got == 0)
			break;
		total += (size_t)got;
	}
	if(total > 0)
		outcome = ANSWERED;
done:
	close(fd);
	return outcome;
}

int main(int argc, char **argv) {
	if(argc < 4 || argc > 6) {
		fprintf(stderr, "usage: connectloop HOST PORT COUNT [PAUSE_MS [REQUEST]]\n");
		return 2;
	}
	int port = atoi(argv[2]);
	int count = atoi(argv[3]);
	long pause_ms = argc > 4 ? atol(argv[4]) : 0;
	struct sockaddr_storage address;
	socklen_t length = make_address(argv[1], port, &address);
	if(!length || port < 1 || port > 65535 || count < 1) {
		fprintf(stderr, "connectloop: HOST is an IPv4 or IPv6 address, PORT is 1 to 65535 and "
				"COUNT is 1 or more\n");
		return 2;
	}
	char request[512];
	size_t request_size = 0;
	if(argc > 5) {
		if(strlen(argv[5]) >= sizeof(request)) {
			fprintf(stderr, "connectloop: REQUEST is at most %zu bytes\n", sizeof(request) - 1);
			return 2;
		}
		request_size = unescape(argv[5], request);
	}
	int refused = 0, failed = 0;
	for(int i = 0; i < count; i++) {
		enum outcome outcome = connect_once(&address, length, request, request_size);
		if(outcome == REFUSED)
			refused++;
		else if(outcome == FAILED)
			failed++;
		if(pause_ms > 0) {
			struct timespec pause = {pause_ms / 1000, (pause_ms % 1000) * 1000000L};
			nanosleep(&pause, NULL);
		}
	}
	printf("connections %d refused %d failed %d\n", count, refused, failed);
	return 0;
}
