// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the sockets boot test runs as a service whose sockets Spark holds
// (ADR 0477): a server that takes every socket in LISTEN_FDS, as
// sd_listen_fds(3) says, and answers each TCP or Unix connection with
// the line `hello from NAME` and each UDP datagram with the same line,
// NAME being the socket's word in LISTEN_FDNAMES. It writes one line to
// its standard output for each socket as it starts, and one for each
// connection or datagram it answers, which the system's log keeps. It
// refuses to run when LISTEN_PID is not its own process: the sockets
// are for the process Spark started.

#include <errno.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/un.h>
#include <netinet/in.h>
#include <unistd.h>

// The most sockets a service file names (SOCKETS_MAX).
#define SOCKETS_MAX 16

// The first descriptor Spark passes (SD_LISTEN_FDS_START).
#define FDS_START 3

struct listener {
	int fd;
	int type;
	char name[80];
};

static struct listener listeners[SOCKETS_MAX];

// Says what `fd` is, in a line of the log: its kind and where it is.
static void describe(const struct listener *listener) {
	struct sockaddr_storage address;
	socklen_t length = sizeof(address);
	if(getsockname(listener->fd, (struct sockaddr *)&address, &length) < 0) {
		printf("listenfd: socket %s: getsockname failed: %s\n", listener->name, strerror(errno));
		return;
	}
	const char *kind = listener->type == SOCK_DGRAM ? "udp" : "tcp";
	if(address.ss_family == AF_UNIX) {
		struct sockaddr_un *unix_address = (struct sockaddr_un *)&address;
		printf("listenfd: socket %s: unix %s\n", listener->name, unix_address->sun_path);
	} else if(address.ss_family == AF_INET6) {
		printf("listenfd: socket %s: %s port %d on IPv6\n", listener->name, kind,
				ntohs(((struct sockaddr_in6 *)&address)->sin6_port));
	} else {
		printf("listenfd: socket %s: %s port %d on IPv4\n", listener->name, kind,
				ntohs(((struct sockaddr_in *)&address)->sin_port));
	}
}

// Takes the sockets in LISTEN_FDS into `listeners`, named by
// LISTEN_FDNAMES, and returns how many; -1 when the environment is not
// Spark's for this process.
static int take_sockets(void) {
	const char *pid = getenv("LISTEN_PID");
	const char *count = getenv("LISTEN_FDS");
	if(!pid || atoi(pid) != getpid() || !count) {
		fprintf(stderr, "listenfd: LISTEN_PID is not this process, or LISTEN_FDS is not set\n");
		return -1;
	}
	int total = atoi(count);
	if(total < 1 || total > SOCKETS_MAX) {
		fprintf(stderr, "listenfd: LISTEN_FDS is %d, not 1 to %d\n", total, SOCKETS_MAX);
		return -1;
	}
	const char *names = getenv("LISTEN_FDNAMES");
	for(int i = 0; i < total; i++) {
		struct listener *listener = &listeners[i];
		listener->fd = FDS_START + i;
		socklen_t length = sizeof(listener->type);
		if(getsockopt(listener->fd, SOL_SOCKET, SO_TYPE, &listener->type, &length) < 0) {
			fprintf(stderr, "listenfd: descriptor %d is no socket: %s\n", listener->fd,
					strerror(errno));
			return -1;
		}
		// The name is the part of LISTEN_FDNAMES between colons; the
		// descriptor's number when it has none.
		snprintf(listener->name, sizeof(listener->name), "%d", listener->fd);
		if(names) {
			const char *start = names;
			for(int skip = 0; skip < i && start; skip++) {
				start = strchr(start, ':');
				if(start)
					start++;
			}
			if(start) {
				size_t size = strcspn(start, ":");
				if(size > 0 && size < sizeof(listener->name)) {
					memcpy(listener->name, start, size);
					listener->name[size] = '\0';
				}
			}
		}
	}
	return total;
}

// Answers the connection waiting on `listener`.
static void answer_connection(const struct listener *listener) {
	int connection = accept(listener->fd, NULL, NULL);
	if(connection < 0) {
		printf("listenfd: socket %s: accept failed: %s\n", listener->name, strerror(errno));
		return;
	}
	char line[128];
	int size = snprintf(line, sizeof(line), "hello from %s\n", listener->name);
	if(write(connection, line, (size_t)size) != size)
		printf("listenfd: socket %s: write failed: %s\n", listener->name, strerror(errno));
	else
		printf("listenfd: socket %s: answered a connection\n", listener->name);
	close(connection);
}

// Answers the datagram waiting on `listener`.
static void answer_datagram(const struct listener *listener) {
	char buffer[512];
	struct sockaddr_storage from;
	socklen_t length = sizeof(from);
	ssize_t got = recvfrom(listener->fd, buffer, sizeof(buffer), 0, (struct sockaddr *)&from,
			&length);
	if(got < 0) {
		printf("listenfd: socket %s: recvfrom failed: %s\n", listener->name, strerror(errno));
		return;
	}
	char line[128];
	int size = snprintf(line, sizeof(line), "hello from %s\n", listener->name);
	if(sendto(listener->fd, line, (size_t)size, 0, (struct sockaddr *)&from, length) != size)
		printf("listenfd: socket %s: sendto failed: %s\n", listener->name, strerror(errno));
	else
		printf("listenfd: socket %s: answered a datagram\n", listener->name);
}

int main(void) {
	setvbuf(stdout, NULL, _IOLBF, 0);
	int total = take_sockets();
	if(total < 0)
		return 1;
	for(int i = 0; i < total; i++)
		describe(&listeners[i]);
	printf("listenfd: serving %d sockets\n", total);
	struct pollfd polls[SOCKETS_MAX];
	for(;;) {
		for(int i = 0; i < total; i++) {
			polls[i].fd = listeners[i].fd;
			polls[i].events = POLLIN;
			polls[i].revents = 0;
		}
		if(poll(polls, (nfds_t)total, -1) < 0) {
			if(errno == EINTR)
				continue;
			printf("listenfd: poll failed: %s\n", strerror(errno));
			return 1;
		}
		for(int i = 0; i < total; i++) {
			if(!(polls[i].revents & POLLIN))
				continue;
			if(listeners[i].type == SOCK_DGRAM)
				answer_datagram(&listeners[i]);
			else
				answer_connection(&listeners[i]);
		}
	}
}
