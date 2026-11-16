// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// AF_UNIX datagram sockets (ADR 0525). Their making, and pairs: datagrams
// kept whole, MSG_TRUNC and MSG_PEEK, FIONREAD, poll, and the socket
// options. Named sockets: bind, connect and AF_UNSPEC, sendto and recvfrom
// with names, and the errors of names that are not a datagram socket's.
// The limits: a datagram's size, and the room a receiver has, which a
// sender waits for. shutdown, a reader that waits for it, and SIGPIPE,
// which a datagram never raises. Descriptors sent in datagrams, and the
// credentials of the sender (SO_PASSCRED, SCM_CREDENTIALS), on both kinds
// of socket. And sockets sent as descriptors, with the refusals that keep
// a socket from holding itself open, and that none leaks when everything
// holding it is closed.

#include <fcntl.h>
#include <poll.h>
#include <stddef.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <sys/un.h>

#include "check.h"

// The names the tests bind, and a file that is no socket's name.
#define NAME "/tmp/datagram-test"
#define OTHER "/tmp/datagram-other"
#define THIRD "/tmp/datagram-third"
#define STREAM "/tmp/datagram-stream"
#define PLAIN "/tmp/datagram-plain"

// What a datagram socket takes (radix-posix(7)): a datagram's size, and how
// many datagrams its queue holds, and how many bytes of them.
#define DATAGRAM_MAX 212960
#define QUEUE_MAX 512
#define QUEUE_BYTES 212992
// A size of which two datagrams fit in a queue's bytes, and three do not.
#define MEDIUM 100000

// How long a part of the test may take, in seconds, before the alarm ends
// the test: every call that waits for something that should be there is
// under it.
#define PATIENCE 60

// Runs part `part` under the alarm.
#define PART(part) \
	do { \
		alarm(PATIENCE); \
		RUN(part); \
		alarm(0); \
	} while(0)

// How many SIGPIPE and SIGUSR1 have come.
static volatile sig_atomic_t pipes;
static volatile sig_atomic_t signals;

static void on_pipe(int number) {
	(void)number;
	pipes++;
}

static void on_signal(int number) {
	(void)number;
	signals++;
}

// An AF_UNIX address for `path`, and its length.
static socklen_t address_of(const char *path, struct sockaddr_un *addr) {
	memset(addr, 0, sizeof(*addr));
	addr->sun_family = AF_UNIX;
	strcpy(addr->sun_path, path);
	return (socklen_t)(offsetof(struct sockaddr_un, sun_path) + strlen(path) + 1);
}

// Whether `name`, of `len` bytes, is `path` (NULL for none: just the
// family).
static int is_name(const struct sockaddr_un *name, socklen_t len, const char *path) {
	if(len < sizeof(sa_family_t) || name->sun_family != AF_UNIX)
		return 0;
	if(!path)
		return len == sizeof(sa_family_t);
	return len == offsetof(struct sockaddr_un, sun_path) + strlen(path) + 1
			&& !strcmp(name->sun_path, path);
}

// Whether `fd`'s name, or with `peer` its peer's, is `path`.
static int named_as(int fd, int peer, const char *path) {
	struct sockaddr_un name;
	memset(&name, 0xff, sizeof(name));
	socklen_t len = sizeof(name);
	int got = peer ? getpeername(fd, (struct sockaddr *)&name, &len)
			: getsockname(fd, (struct sockaddr *)&name, &len);
	return got == 0 && is_name(&name, len, path);
}

// A datagram socket bound to `path`, or, with NULL, to no name.
static int datagram(const char *path) {
	int s = socket(AF_UNIX, SOCK_DGRAM, 0);
	CHECK(s >= 0);
	if(path) {
		struct sockaddr_un addr;
		socklen_t len = address_of(path, &addr);
		CHECK(bind(s, (struct sockaddr *)&addr, len) == 0);
	}
	return s;
}

// Connects `fd` to the socket named `path`.
static int connect_to(int fd, const char *path) {
	struct sockaddr_un addr;
	socklen_t len = address_of(path, &addr);
	return connect(fd, (struct sockaddr *)&addr, len);
}

// Sends `text` from `fd` to the socket named `path`.
static ssize_t send_to(int fd, const char *path, const char *text) {
	struct sockaddr_un addr;
	socklen_t len = address_of(path, &addr);
	return sendto(fd, text, strlen(text), 0, (struct sockaddr *)&addr, len);
}

// Whether `text` went from `fd` in one datagram, all of it.
static int sent(int fd, const char *text) {
	return send(fd, text, strlen(text), 0) == (ssize_t)strlen(text);
}

// Whether `text` went from `fd` to the socket named `path`.
static int sent_to(int fd, const char *path, const char *text) {
	return send_to(fd, path, text) == (ssize_t)strlen(text);
}

// Whether the next datagram of `fd`, received with `flags`, is `text`.
static int received(int fd, int flags, const char *text) {
	char buffer[256];
	return recv(fd, buffer, sizeof(buffer), flags) == (ssize_t)strlen(text)
			&& !memcmp(buffer, text, strlen(text));
}

// Whether the next datagram of `fd` is `text`, sent from the socket named
// `path`, or from one with no name if `path` is NULL.
static int received_from(int fd, const char *text, const char *path) {
	char buffer[256];
	struct sockaddr_un from;
	socklen_t from_len = sizeof(from);
	ssize_t got = recvfrom(fd, buffer, sizeof(buffer), 0, (struct sockaddr *)&from, &from_len);
	return got == (ssize_t)strlen(text) && !memcmp(buffer, text, strlen(text))
			&& (path ? is_name(&from, from_len, path) : from_len == 0);
}

// Sends the `size` bytes at `data` from `fd` as one datagram, to the
// socket named `to` (NULL for the one `fd` is connected to), with the
// `count` descriptors of `fds` (at most 8) and, if `credentials` is not
// NULL, a SCM_CREDENTIALS message of it.
static ssize_t send_message(int fd, const char *to, const void *data, size_t size,
		const int *fds, int count, const struct ucred *credentials) {
	union {
		struct cmsghdr header;
		char room[CMSG_SPACE(8 * sizeof(int)) + CMSG_SPACE(sizeof(struct ucred))];
	} control;
	memset(&control, 0, sizeof(control));
	struct sockaddr_un name;
	struct iovec iov = {(void *)data, size};
	struct msghdr message = {.msg_iov = &iov, .msg_iovlen = 1};
	if(to) {
		message.msg_name = &name;
		message.msg_namelen = address_of(to, &name);
	}
	size_t room = (count ? CMSG_SPACE(count * sizeof(int)) : 0)
			+ (credentials ? CMSG_SPACE(sizeof(*credentials)) : 0);
	if(room) {
		message.msg_control = control.room;
		message.msg_controllen = room;
	}
	struct cmsghdr *cmsg = CMSG_FIRSTHDR(&message);
	if(count) {
		cmsg->cmsg_level = SOL_SOCKET;
		cmsg->cmsg_type = SCM_RIGHTS;
		cmsg->cmsg_len = CMSG_LEN(count * sizeof(int));
		memcpy(CMSG_DATA(cmsg), fds, count * sizeof(int));
		cmsg = CMSG_NXTHDR(&message, cmsg);
	}
	if(credentials) {
		cmsg->cmsg_level = SOL_SOCKET;
		cmsg->cmsg_type = SCM_CREDENTIALS;
		cmsg->cmsg_len = CMSG_LEN(sizeof(*credentials));
		memcpy(CMSG_DATA(cmsg), credentials, sizeof(*credentials));
	}
	return sendmsg(fd, &message, 0);
}

// What one recvmsg gave.
struct received {
	// The bytes, or -1 and the error.
	ssize_t got;
	int error;
	// The message's flags.
	int flags;
	// The descriptors that came, and their count.
	int fds[8];
	int count;
	// The type of the first control message, or 0 for none.
	int first;
	// The credentials that came, if any did.
	int has_credentials;
	struct ucred credentials;
	// How much control data came.
	size_t control_len;
	// The name of the sender, and its length.
	struct sockaddr_un from;
	socklen_t from_len;
};

// Receives one message from `fd` into `buffer`, with `control` bytes of
// room for descriptors and credentials (at most what both need).
static void receive(int fd, void *buffer, size_t size, size_t control, int flags,
		struct received *r) {
	union {
		struct cmsghdr header;
		char room[CMSG_SPACE(8 * sizeof(int)) + CMSG_SPACE(sizeof(struct ucred))];
	} space;
	memset(&space, 0, sizeof(space));
	memset(r, 0, sizeof(*r));
	struct iovec iov = {buffer, size};
	struct msghdr message = {.msg_iov = &iov, .msg_iovlen = 1};
	message.msg_name = &r->from;
	message.msg_namelen = sizeof(r->from);
	if(control) {
		message.msg_control = space.room;
		message.msg_controllen = control;
	}
	r->got = recvmsg(fd, &message, flags);
	r->error = r->got < 0 ? errno : 0;
	r->flags = message.msg_flags;
	r->from_len = message.msg_namelen;
	r->control_len = message.msg_controllen;
	if(r->got < 0)
		return;
	for(struct cmsghdr *cmsg = CMSG_FIRSTHDR(&message); cmsg; cmsg = CMSG_NXTHDR(&message, cmsg)) {
		if(!r->first)
			r->first = cmsg->cmsg_type;
		if(cmsg->cmsg_level != SOL_SOCKET)
			continue;
		if(cmsg->cmsg_type == SCM_RIGHTS) {
			int n = (int)((cmsg->cmsg_len - CMSG_LEN(0)) / sizeof(int));
			CHECK(r->count + n <= 8);
			memcpy(r->fds + r->count, CMSG_DATA(cmsg), n * sizeof(int));
			r->count += n;
		} else if(cmsg->cmsg_type == SCM_CREDENTIALS
				&& cmsg->cmsg_len == CMSG_LEN(sizeof(struct ucred))) {
			r->has_credentials = 1;
			memcpy(&r->credentials, CMSG_DATA(cmsg), sizeof(struct ucred));
		}
	}
}

// Whether `fd`'s SO_PEERCRED is process `pid`, with this process's user
// and group.
static int peer_is(int fd, pid_t pid) {
	struct ucred cred;
	socklen_t size = sizeof(cred);
	return getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &cred, &size) == 0 && size == sizeof(cred)
			&& cred.pid == pid && cred.uid == getuid() && cred.gid == getgid();
}

// Whether `fd`'s SO_PEERCRED is Linux's for no peer: process 0, and no
// user and no group, which is not user 0 (root).
static int peer_unset(int fd) {
	struct ucred cred;
	socklen_t size = sizeof(cred);
	return getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &cred, &size) == 0 && size == sizeof(cred)
			&& cred.pid == 0 && cred.uid == (uid_t)-1 && cred.gid == (gid_t)-1;
}

static void made(void) {
	int fd = socket(AF_UNIX, SOCK_DGRAM, 0);
	CHECK(fd >= 0);
	struct stat st;
	CHECK(fstat(fd, &st) == 0 && S_ISSOCK(st.st_mode));
	// Neither named nor connected.
	CHECK(named_as(fd, 0, NULL));
	struct sockaddr_un name;
	socklen_t len = sizeof(name);
	CHECK_ERROR(getpeername(fd, (struct sockaddr *)&name, &len), ENOTCONN);
	CHECK_ERROR(send(fd, "x", 1, 0), ENOTCONN);
	char byte;
	CHECK_ERROR(recv(fd, &byte, 1, MSG_DONTWAIT), EAGAIN);
	int pending = -1;
	CHECK(ioctl(fd, FIONREAD, &pending) == 0 && pending == 0);
	struct pollfd poller = {.fd = fd, .events = POLLIN | POLLOUT};
	CHECK(poll(&poller, 1, 0) == 1 && poller.revents == POLLOUT);
	// A datagram socket neither listens nor accepts.
	CHECK_ERROR(listen(fd, 1), EOPNOTSUPP);
	CHECK_ERROR(accept(fd, NULL, NULL), EOPNOTSUPP);
	int value = -1;
	socklen_t size = sizeof(value);
	CHECK(getsockopt(fd, SOL_SOCKET, SO_ACCEPTCONN, &value, &size) == 0 && value == 0);
	close(fd);
	// Non-blocking and close-on-exec, as the type says.
	fd = socket(AF_UNIX, SOCK_DGRAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
	CHECK(fd >= 0);
	CHECK(fcntl(fd, F_GETFL) & O_NONBLOCK);
	CHECK(fcntl(fd, F_GETFD) & FD_CLOEXEC);
	CHECK_ERROR(read(fd, &byte, 1), EAGAIN);
	CHECK_ERROR(recv(fd, &byte, 1, 0), EAGAIN);
	close(fd);
	int p[2];
	CHECK(socketpair(AF_UNIX, SOCK_DGRAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0, p) == 0);
	CHECK(fcntl(p[0], F_GETFL) & O_NONBLOCK);
	CHECK(fcntl(p[1], F_GETFD) & FD_CLOEXEC);
	CHECK_ERROR(recv(p[0], &byte, 1, 0), EAGAIN);
	close(p[0]);
	close(p[1]);
	// The protocol is the family's, or none.
	fd = socket(AF_UNIX, SOCK_DGRAM, PF_UNIX);
	CHECK(fd >= 0);
	close(fd);
	CHECK_ERROR(socket(AF_UNIX, SOCK_DGRAM, 99), EPROTONOSUPPORT);
	CHECK_ERROR(socketpair(AF_UNIX, SOCK_DGRAM, 99, p), EPROTONOSUPPORT);
	// Sequenced-packet sockets are not made.
	CHECK_ERROR(socket(AF_UNIX, SOCK_SEQPACKET, 0), ESOCKTNOSUPPORT);
	CHECK_ERROR(socketpair(AF_UNIX, SOCK_SEQPACKET, 0, p), ESOCKTNOSUPPORT);
}

static void pair(void) {
	int p[2];
	CHECK(socketpair(AF_UNIX, SOCK_DGRAM, 0, p) == 0);
	CHECK(p[0] >= 0 && p[1] >= 0 && p[0] != p[1]);
	// Each end gets what the other sends, by whichever call.
	CHECK(sent(p[0], "ping"));
	CHECK(write(p[1], "pong!", strlen("pong!")) == (ssize_t)strlen("pong!"));
	char buffer[16] = {0};
	CHECK(read(p[1], buffer, sizeof(buffer)) == (ssize_t)strlen("ping")
			&& !memcmp(buffer, "ping", strlen("ping")));
	CHECK(received(p[0], 0, "pong!"));
	// A receive takes one datagram, whatever room the buffer has for more.
	CHECK(sent(p[0], "one"));
	CHECK(sent(p[0], "two!"));
	CHECK(received(p[1], 0, "one"));
	CHECK(received(p[1], 0, "two!"));
	CHECK_ERROR(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	// One of no bytes is a datagram too.
	CHECK(sent(p[0], ""));
	CHECK(received(p[1], 0, ""));
	CHECK_ERROR(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	// A receive into no room takes a datagram all the same, and MSG_PEEK
	// with MSG_TRUNC gives the size of the next one.
	CHECK(sent(p[0], "lost"));
	CHECK(recv(p[1], buffer, 0, 0) == 0);
	CHECK(sent(p[0], "size"));
	CHECK(recv(p[1], NULL, 0, MSG_PEEK | MSG_TRUNC) == (ssize_t)strlen("size"));
	CHECK(recv(p[1], NULL, 0, MSG_TRUNC) == (ssize_t)strlen("size"));
	CHECK_ERROR(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	// What the buffer has no room for is lost, and MSG_TRUNC says how big
	// the datagram was.
	static const char digits[] = "0123456789";
	char small[4];
	struct received r;
	CHECK(sent(p[0], digits));
	CHECK(recv(p[1], small, sizeof(small), 0) == (ssize_t)sizeof(small)
			&& !memcmp(small, digits, sizeof(small)));
	CHECK_ERROR(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	CHECK(sent(p[0], digits));
	CHECK(recv(p[1], small, sizeof(small), MSG_TRUNC) == (ssize_t)strlen(digits)
			&& !memcmp(small, digits, sizeof(small)));
	CHECK_ERROR(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	CHECK(sent(p[0], digits));
	receive(p[1], small, sizeof(small), 0, 0, &r);
	CHECK(r.got == (ssize_t)sizeof(small) && (r.flags & MSG_TRUNC));
	// A datagram that fits has no flag, and MSG_TRUNC gives its size.
	CHECK(sent(p[0], "abcd"));
	receive(p[1], small, sizeof(small), 0, 0, &r);
	CHECK(r.got == (ssize_t)sizeof(small) && !(r.flags & MSG_TRUNC));
	CHECK(sent(p[0], "abcd"));
	CHECK(recv(p[1], buffer, sizeof(buffer), MSG_TRUNC) == (ssize_t)strlen("abcd"));
	// MSG_PEEK leaves the datagram, all of it, whatever the peek took.
	CHECK(sent(p[0], "peeked"));
	CHECK(recv(p[1], small, 2, MSG_PEEK) == 2 && !memcmp(small, "pe", 2));
	CHECK(received(p[1], MSG_PEEK, "peeked"));
	CHECK(received(p[1], 0, "peeked"));
	CHECK_ERROR(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	// FIONREAD is the size of the next datagram.
	int pending = -1;
	CHECK(ioctl(p[1], FIONREAD, &pending) == 0 && pending == 0);
	CHECK(sent(p[0], "1234567"));
	CHECK(sent(p[0], "abc"));
	CHECK(ioctl(p[1], FIONREAD, &pending) == 0 && pending == (int)strlen("1234567"));
	CHECK(received(p[1], 0, "1234567"));
	CHECK(ioctl(p[1], FIONREAD, &pending) == 0 && pending == (int)strlen("abc"));
	CHECK(received(p[1], 0, "abc"));
	CHECK(ioctl(p[1], FIONREAD, &pending) == 0 && pending == 0);
	// Writable until the other end's queue is full, readable while a
	// datagram waits.
	struct pollfd poller = {.fd = p[1], .events = POLLIN | POLLOUT};
	CHECK(poll(&poller, 1, 0) == 1 && poller.revents == POLLOUT);
	CHECK(sent(p[0], "x"));
	CHECK(poll(&poller, 1, 0) == 1 && poller.revents == (POLLIN | POLLOUT));
	CHECK(received(p[1], 0, "x"));
	// What the sockets say of themselves.
	struct stat st;
	CHECK(fstat(p[0], &st) == 0 && S_ISSOCK(st.st_mode));
	int value = -1;
	socklen_t size = sizeof(value);
	CHECK(getsockopt(p[0], SOL_SOCKET, SO_TYPE, &value, &size) == 0);
	CHECK(value == SOCK_DGRAM && size == sizeof(int));
	CHECK(getsockopt(p[0], SOL_SOCKET, SO_DOMAIN, &value, &size) == 0 && value == AF_UNIX);
	CHECK(getsockopt(p[0], SOL_SOCKET, SO_ERROR, &value, &size) == 0 && value == 0);
	CHECK(getsockopt(p[0], SOL_SOCKET, SO_ACCEPTCONN, &value, &size) == 0 && value == 0);
	CHECK(getsockopt(p[0], SOL_SOCKET, SO_RCVBUF, &value, &size) == 0 && value == QUEUE_BYTES);
	CHECK(getsockopt(p[0], SOL_SOCKET, SO_SNDBUF, &value, &size) == 0 && value == QUEUE_BYTES);
	// A pair is each end's peer, which neither has a name for.
	CHECK(named_as(p[0], 0, NULL));
	CHECK(named_as(p[0], 1, NULL));
	CHECK(peer_is(p[0], getpid()) && peer_is(p[1], getpid()));
	CHECK_ERROR(lseek(p[0], 0, SEEK_SET), ESPIPE);
	close(p[0]);
	close(p[1]);
}

// sendmsg gathers its buffers into one datagram and recvmsg scatters one.
static void vectors(void) {
	int p[2];
	CHECK(socketpair(AF_UNIX, SOCK_DGRAM, 0, p) == 0);
	static const char one[] = "one ";
	static const char two[] = "two three";
	struct iovec out[3] = {{(void *)one, strlen(one)}, {(void *)"", 0}, {(void *)two, strlen(two)}};
	struct msghdr message = {.msg_iov = out, .msg_iovlen = 3};
	ssize_t whole = (ssize_t)(strlen(one) + strlen(two));
	CHECK(sendmsg(p[0], &message, 0) == whole);
	CHECK(sendmsg(p[0], &message, 0) == whole);
	char first[5] = {0};
	char second[16] = {0};
	struct iovec in[2] = {{first, strlen(one)}, {second, sizeof(second)}};
	struct msghdr scattered = {.msg_iov = in, .msg_iovlen = 2};
	scattered.msg_flags = -1;
	CHECK(recvmsg(p[1], &scattered, 0) == whole);
	CHECK(!memcmp(first, one, strlen(one)) && !memcmp(second, two, strlen(two)));
	CHECK(scattered.msg_flags == 0 && scattered.msg_controllen == 0);
	// The datagram takes the buffers it has room in, and the rest of it is
	// lost, which MSG_TRUNC says.
	char small[3] = {0};
	struct iovec short_in[2] = {{small, sizeof(small)}, {second, 2}};
	struct msghdr cut = {.msg_iov = short_in, .msg_iovlen = 2};
	CHECK(recvmsg(p[1], &cut, 0) == (ssize_t)(sizeof(small) + 2));
	CHECK(!memcmp(small, "one", sizeof(small)) && !memcmp(second, " t", 2));
	CHECK(cut.msg_flags & MSG_TRUNC);
	CHECK_ERROR(recvmsg(p[1], &scattered, MSG_DONTWAIT), EAGAIN);
	// MSG_WAITALL does not wait for more datagrams.
	char buffer[16];
	CHECK(sent(p[0], "abc"));
	CHECK(recv(p[1], buffer, sizeof(buffer), MSG_WAITALL) == (ssize_t)strlen("abc"));
	// No buffer sends a datagram of no bytes.
	struct msghdr none = {0};
	CHECK(sendmsg(p[0], &none, 0) == 0);
	CHECK(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT) == 0);
	CHECK_ERROR(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	close(p[0]);
	close(p[1]);
}

static void named(void) {
	unlink(NAME);
	unlink(OTHER);
	unlink(THIRD);
	unlink(STREAM);
	unlink(PLAIN);
	int receiver = datagram(NAME);
	CHECK(named_as(receiver, 0, NAME));
	struct sockaddr_un addr;
	socklen_t len = sizeof(addr);
	CHECK_ERROR(getpeername(receiver, (struct sockaddr *)&addr, &len), ENOTCONN);
	struct stat st;
	CHECK(stat(NAME, &st) == 0 && S_ISSOCK(st.st_mode));
	// A name is taken once, and a socket has one name.
	int other = socket(AF_UNIX, SOCK_DGRAM, 0);
	len = address_of(NAME, &addr);
	CHECK_ERROR(bind(other, (struct sockaddr *)&addr, len), EADDRINUSE);
	len = address_of(OTHER, &addr);
	CHECK(bind(other, (struct sockaddr *)&addr, len) == 0);
	socklen_t again_len = address_of("/tmp/datagram-again", &addr);
	CHECK_ERROR(bind(other, (struct sockaddr *)&addr, again_len), EINVAL);
	// Datagrams to an unconnected socket, from a socket with no name...
	int anon = datagram(NULL);
	char buffer[32];
	CHECK(sent_to(anon, NAME, "hello"));
	CHECK(received_from(receiver, "hello", NULL));
	// ...and from one with, which the receiver answers by name.
	CHECK(sent_to(other, NAME, "named"));
	struct sockaddr_un from;
	socklen_t from_len = sizeof(from);
	CHECK(recvfrom(receiver, buffer, sizeof(buffer), 0, (struct sockaddr *)&from, &from_len)
			== (ssize_t)strlen("named"));
	CHECK(!memcmp(buffer, "named", strlen("named")) && is_name(&from, from_len, OTHER));
	CHECK(sendto(receiver, "reply", strlen("reply"), 0, (struct sockaddr *)&from, from_len)
			== (ssize_t)strlen("reply"));
	CHECK(received(other, 0, "reply"));
	// The same by sendmsg and recvmsg.
	struct sockaddr_un to;
	socklen_t to_len = address_of(NAME, &to);
	static const char text[] = "viamsg";
	struct iovec out = {(void *)text, strlen(text)};
	struct msghdr message = {.msg_name = &to, .msg_namelen = to_len, .msg_iov = &out,
			.msg_iovlen = 1};
	CHECK(sendmsg(other, &message, 0) == (ssize_t)strlen(text));
	struct iovec in = {buffer, sizeof(buffer)};
	struct msghdr incoming = {.msg_name = &from, .msg_namelen = sizeof(from), .msg_iov = &in,
			.msg_iovlen = 1};
	CHECK(recvmsg(receiver, &incoming, 0) == (ssize_t)strlen(text));
	CHECK(!memcmp(buffer, text, strlen(text)) && is_name(&from, incoming.msg_namelen, OTHER));
	// Without a name, an unconnected socket has nowhere to send.
	CHECK_ERROR(send(anon, "x", 1, 0), ENOTCONN);
	// connect sets where send goes, and from whom the socket takes
	// datagrams.
	CHECK(connect_to(other, NAME) == 0);
	CHECK(named_as(other, 1, NAME));
	CHECK(sent(other, "connected"));
	CHECK(received_from(receiver, "connected", OTHER));
	CHECK_ERROR(send_to(anon, OTHER, "no"), EPERM);
	CHECK(sent_to(receiver, OTHER, "yes"));
	CHECK(received(other, 0, "yes"));
	// It may connect again, which throws away what waited for it.
	int third = datagram(THIRD);
	CHECK(sent_to(receiver, OTHER, "stale"));
	CHECK(connect_to(other, THIRD) == 0);
	CHECK(named_as(other, 1, THIRD));
	CHECK_ERROR(recv(other, buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	CHECK(sent(other, "third"));
	CHECK(received(third, 0, "third"));
	// The old peer is not its peer any more.
	CHECK_ERROR(send_to(receiver, OTHER, "late"), EPERM);
	// A socket connected by name has no credentials of a peer to give, as
	// an unconnected one has none; a pair's end has its maker's.
	CHECK(peer_unset(other));
	CHECK(peer_unset(anon));
	// AF_UNSPEC makes it unconnected: what waits for it is thrown away,
	// and it takes datagrams from anyone.
	CHECK(sent_to(third, OTHER, "dropped"));
	struct sockaddr unspec = {.sa_family = AF_UNSPEC};
	CHECK(connect(other, &unspec, sizeof(unspec)) == 0);
	CHECK_ERROR(getpeername(other, (struct sockaddr *)&addr, &len), ENOTCONN);
	CHECK_ERROR(send(other, "x", 1, 0), ENOTCONN);
	CHECK_ERROR(recv(other, buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	CHECK(sent_to(anon, OTHER, "again"));
	CHECK(received(other, 0, "again"));
	CHECK(connect(other, &unspec, sizeof(unspec)) == 0);
	// A name with no socket behind it is not found; a file that is no
	// socket's name, and a socket that has gone, refuse.
	CHECK_ERROR(connect_to(anon, "/tmp/datagram-missing"), ENOENT);
	CHECK_ERROR(send_to(anon, "/tmp/datagram-missing", "x"), ENOENT);
	int plain = open(PLAIN, O_WRONLY | O_CREAT | O_TRUNC, 0644);
	CHECK(plain >= 0);
	close(plain);
	CHECK_ERROR(connect_to(anon, PLAIN), ECONNREFUSED);
	CHECK_ERROR(send_to(anon, PLAIN, "x"), ECONNREFUSED);
	// A stream socket's name is of the other type, as a datagram socket's
	// is to a stream socket.
	int listener = socket(AF_UNIX, SOCK_STREAM, 0);
	len = address_of(STREAM, &addr);
	CHECK(bind(listener, (struct sockaddr *)&addr, len) == 0);
	CHECK(listen(listener, 1) == 0);
	CHECK_ERROR(connect_to(anon, STREAM), EPROTOTYPE);
	CHECK_ERROR(send_to(anon, STREAM, "x"), EPROTOTYPE);
	int client = socket(AF_UNIX, SOCK_STREAM, 0);
	CHECK_ERROR(connect_to(client, NAME), EPROTOTYPE);
	// A stream socket sends no datagram to a name, and has no AF_UNSPEC.
	CHECK_ERROR(send_to(client, NAME, "x"), EOPNOTSUPP);
	CHECK_ERROR(connect(client, &unspec, sizeof(unspec)), EINVAL);
	close(client);
	close(listener);
	unlink(STREAM);
	// A socket that has gone leaves its name behind.
	close(third);
	CHECK(stat(THIRD, &st) == 0 && S_ISSOCK(st.st_mode));
	CHECK_ERROR(connect_to(anon, THIRD), ECONNREFUSED);
	CHECK_ERROR(send_to(anon, THIRD, "x"), ECONNREFUSED);
	// A connected socket whose peer has gone is told once, and is then not
	// connected.
	CHECK(connect_to(other, NAME) == 0);
	close(receiver);
	CHECK_ERROR(send(other, "x", 1, 0), ECONNREFUSED);
	CHECK_ERROR(send(other, "x", 1, 0), ENOTCONN);
	CHECK_ERROR(getpeername(other, (struct sockaddr *)&addr, &len), ENOTCONN);
	close(other);
	close(anon);
	unlink(NAME);
	unlink(OTHER);
	unlink(THIRD);
	unlink(PLAIN);
}

// How many datagrams of one byte `fd` takes before it would wait.
static int fill(int fd) {
	int count = 0;
	while(count <= QUEUE_MAX + 8 && send(fd, "q", 1, MSG_DONTWAIT) == 1)
		count++;
	return count;
}

static char big[DATAGRAM_MAX + 1];
static char back[DATAGRAM_MAX + 1];

static void limits(void) {
	int p[2];
	CHECK(socketpair(AF_UNIX, SOCK_DGRAM, 0, p) == 0);
	for(size_t i = 0; i < sizeof(big); i++)
		big[i] = (char)(i % 251);
	// A datagram is at most DATAGRAM_MAX bytes: one more is too many, and
	// the biggest goes.
	CHECK_ERROR(send(p[0], big, DATAGRAM_MAX + 1, MSG_DONTWAIT), EMSGSIZE);
	CHECK_ERROR(send(p[0], big, DATAGRAM_MAX + 1, 0), EMSGSIZE);
	struct iovec halves[2] = {{big, MEDIUM}, {big + MEDIUM, DATAGRAM_MAX + 1 - MEDIUM}};
	struct msghdr message = {.msg_iov = halves, .msg_iovlen = 2};
	CHECK_ERROR(sendmsg(p[0], &message, 0), EMSGSIZE);
	CHECK_ERROR(recv(p[1], back, sizeof(back), MSG_DONTWAIT), EAGAIN);
	CHECK(send(p[0], big, DATAGRAM_MAX, 0) == DATAGRAM_MAX);
	CHECK(recv(p[1], back, sizeof(back), MSG_DONTWAIT) == DATAGRAM_MAX);
	CHECK(!memcmp(back, big, DATAGRAM_MAX));
	// A queue takes 212,992 bytes of datagrams, but one datagram always goes
	// into an empty queue: the biggest fills it, and the next has to wait.
	CHECK(send(p[0], big, DATAGRAM_MAX, 0) == DATAGRAM_MAX);
	CHECK_ERROR(send(p[0], big, DATAGRAM_MAX, MSG_DONTWAIT), EAGAIN);
	struct pollfd poller = {.fd = p[0], .events = POLLOUT};
	CHECK(poll(&poller, 1, 0) == 0);
	CHECK(recv(p[1], back, sizeof(back), MSG_DONTWAIT) == DATAGRAM_MAX);
	CHECK(poll(&poller, 1, 0) == 1 && poller.revents == POLLOUT);
	CHECK(send(p[0], big, DATAGRAM_MAX, MSG_DONTWAIT) == DATAGRAM_MAX);
	CHECK(recv(p[1], back, sizeof(back), MSG_DONTWAIT) == DATAGRAM_MAX);
	// Smaller ones go until their bytes would be too many.
	CHECK(send(p[0], big, MEDIUM, MSG_DONTWAIT) == MEDIUM);
	CHECK(send(p[0], big, MEDIUM, MSG_DONTWAIT) == MEDIUM);
	CHECK_ERROR(send(p[0], big, MEDIUM, MSG_DONTWAIT), EAGAIN);
	CHECK(recv(p[1], back, sizeof(back), MSG_DONTWAIT) == MEDIUM);
	CHECK(recv(p[1], back, sizeof(back), MSG_DONTWAIT) == MEDIUM);
	// A poll says a socket can send while its peer's queue has at most a
	// quarter of its bytes taken, as Linux says while a socket has sent at
	// most a quarter of its buffer.
	CHECK(send(p[0], big, QUEUE_BYTES / 4, MSG_DONTWAIT) == QUEUE_BYTES / 4);
	CHECK(poll(&poller, 1, 0) == 1 && poller.revents == POLLOUT);
	CHECK(send(p[0], "+", 1, MSG_DONTWAIT) == 1);
	CHECK(poll(&poller, 1, 0) == 0);
	CHECK(recv(p[1], back, sizeof(back), MSG_DONTWAIT) == QUEUE_BYTES / 4);
	CHECK(poll(&poller, 1, 0) == 1 && poller.revents == POLLOUT);
	CHECK(recv(p[1], back, sizeof(back), MSG_DONTWAIT) == 1);
	// And QUEUE_MAX datagrams, of however few bytes.
	CHECK(fill(p[0]) == QUEUE_MAX && errno == EAGAIN);
	CHECK(poll(&poller, 1, 0) == 0);
	// A receive makes room for one more, and no more.
	char buffer[16];
	CHECK(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT) == 1);
	CHECK(poll(&poller, 1, 0) == 1 && poller.revents == POLLOUT);
	CHECK(send(p[0], "q", 1, MSG_DONTWAIT) == 1);
	CHECK_ERROR(send(p[0], "q", 1, MSG_DONTWAIT), EAGAIN);
	int count = 0;
	while(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT) == 1 && buffer[0] == 'q')
		count++;
	CHECK(count == QUEUE_MAX && errno == EAGAIN);
	// A sender that finds the queue full waits for room.
	CHECK(fill(p[0]) == QUEUE_MAX);
	pid_t pid = check_fork();
	if(pid == 0) {
		alarm(PATIENCE);
		CHECK(sent(p[0], "late"));
		check_exit_child();
	}
	check_sleep_ms(200);
	CHECK(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT) == 1);
	CHECK_CHILD(pid);
	count = 0;
	ssize_t got;
	while((got = recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT)) == 1)
		count++;
	CHECK(count == QUEUE_MAX - 1 && got == (ssize_t)strlen("late")
			&& !memcmp(buffer, "late", strlen("late")));
	// A signal ends that wait, as it ends a pipe's.
	CHECK(fill(p[0]) == QUEUE_MAX);
	struct sigaction action = {.sa_handler = on_signal};
	CHECK(sigaction(SIGUSR1, &action, NULL) == 0);
	pid = check_fork();
	if(pid == 0) {
		alarm(PATIENCE);
		CHECK_ERROR(send(p[0], "x", 1, 0), EINTR);
		CHECK(signals == 1);
		check_exit_child();
	}
	check_sleep_ms(300);
	CHECK(kill(pid, SIGUSR1) == 0);
	CHECK_CHILD(pid);
	signal(SIGUSR1, SIG_DFL);
	while(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT) == 1) {
	}
	// A receiver that finds none waits for one, and a datagram of no bytes
	// ends the wait too.
	pid = check_fork();
	if(pid == 0) {
		alarm(PATIENCE);
		CHECK(received(p[1], 0, "wake"));
		CHECK(recv(p[1], buffer, sizeof(buffer), 0) == 0);
		check_exit_child();
	}
	check_sleep_ms(200);
	CHECK(sent(p[0], "wake"));
	check_sleep_ms(200);
	CHECK(sent(p[0], ""));
	CHECK_CHILD(pid);
	close(p[0]);
	close(p[1]);
}

static void shutting(void) {
	CHECK(signal(SIGPIPE, on_pipe) != SIG_ERR);
	pipes = 0;
	int p[2];
	CHECK(socketpair(AF_UNIX, SOCK_DGRAM, 0, p) == 0);
	char buffer[16];
	// Shut for writing: sends fail with EPIPE, and a datagram raises no
	// SIGPIPE.
	CHECK(sent(p[0], "before"));
	CHECK(shutdown(p[0], SHUT_WR) == 0);
	CHECK_ERROR(send(p[0], "x", 1, 0), EPIPE);
	CHECK_ERROR(send(p[0], "x", 1, MSG_NOSIGNAL), EPIPE);
	CHECK(pipes == 0);
	// What was sent before is there, and the other end can send.
	CHECK(received(p[1], MSG_DONTWAIT, "before"));
	CHECK(sent(p[1], "back"));
	CHECK(received(p[0], MSG_DONTWAIT, "back"));
	CHECK(shutdown(p[0], SHUT_WR) == 0);
	close(p[0]);
	close(p[1]);
	// Shut for reading, with a datagram waiting: it is received, and then
	// a receive that waits finds the end of the socket, and returns 0.
	CHECK(socketpair(AF_UNIX, SOCK_DGRAM, 0, p) == 0);
	CHECK(sent(p[0], "queued"));
	CHECK(shutdown(p[1], SHUT_RD) == 0);
	struct pollfd poller = {.fd = p[1], .events = POLLIN};
	CHECK(poll(&poller, 1, 0) == 1 && (poller.revents & POLLIN));
	CHECK(received(p[1], 0, "queued"));
	CHECK(recv(p[1], buffer, sizeof(buffer), 0) == 0);
	CHECK(poll(&poller, 1, 0) == 1 && (poller.revents & POLLIN));
	// One that does not wait finds nothing, as with nothing to wait for.
	CHECK_ERROR(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	int flags = fcntl(p[1], F_GETFL);
	CHECK(fcntl(p[1], F_SETFL, flags | O_NONBLOCK) == 0);
	CHECK_ERROR(recv(p[1], buffer, sizeof(buffer), 0), EAGAIN);
	CHECK_ERROR(read(p[1], buffer, sizeof(buffer)), EAGAIN);
	CHECK(fcntl(p[1], F_SETFL, flags) == 0);
	// A sender to it gets EPIPE, and no signal; its own end is not shut.
	CHECK_ERROR(send(p[0], "x", 1, 0), EPIPE);
	CHECK(pipes == 0);
	CHECK_ERROR(recv(p[0], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	poller.fd = p[0];
	poller.events = POLLIN;
	CHECK(poll(&poller, 1, 0) == 0);
	// Both ways: hung up.
	CHECK(shutdown(p[1], SHUT_WR) == 0);
	poller.fd = p[1];
	poller.events = POLLIN | POLLOUT;
	CHECK(poll(&poller, 1, 0) == 1 && (poller.revents & POLLHUP));
	close(p[0]);
	close(p[1]);
	// A receiver that waits when its socket is shut for reading wakes.
	CHECK(socketpair(AF_UNIX, SOCK_DGRAM, 0, p) == 0);
	pid_t pid = check_fork();
	if(pid == 0) {
		alarm(PATIENCE);
		CHECK(recv(p[1], buffer, sizeof(buffer), 0) == 0);
		check_exit_child();
	}
	check_sleep_ms(200);
	CHECK(shutdown(p[1], SHUT_RD) == 0);
	CHECK_CHILD(pid);
	close(p[0]);
	close(p[1]);
	// Neither connected nor named, a socket shuts too.
	int fd = datagram(NULL);
	CHECK(shutdown(fd, SHUT_RD) == 0);
	CHECK(shutdown(fd, SHUT_WR) == 0);
	CHECK_ERROR(shutdown(fd, 7), EINVAL);
	close(fd);
	// A peer that is closed ends nothing: the other end's receive goes on
	// waiting, and its sends are refused once.
	CHECK(socketpair(AF_UNIX, SOCK_DGRAM, 0, p) == 0);
	close(p[1]);
	CHECK_ERROR(recv(p[0], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	CHECK_ERROR(send(p[0], "x", 1, 0), ECONNREFUSED);
	CHECK_ERROR(send(p[0], "x", 1, 0), ENOTCONN);
	CHECK(pipes == 0);
	close(p[0]);
	CHECK(signal(SIGPIPE, SIG_DFL) != SIG_ERR);
}

static void descriptors(void) {
	int p[2];
	CHECK(socketpair(AF_UNIX, SOCK_DGRAM, 0, p) == 0);
	// A pipe's write end: what goes in it comes out of the read end.
	int pipe_fds[2];
	CHECK(pipe(pipe_fds) == 0);
	CHECK(send_message(p[0], NULL, "p", 1, &pipe_fds[1], 1, NULL) == 1);
	CHECK(close(pipe_fds[1]) == 0);
	char buffer[16];
	struct received r;
	receive(p[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && buffer[0] == 'p' && r.count == 1 && r.flags == 0);
	CHECK(!(fcntl(r.fds[0], F_GETFD) & FD_CLOEXEC));
	CHECK(write(r.fds[0], "through", strlen("through")) == (ssize_t)strlen("through"));
	CHECK(read(pipe_fds[0], buffer, sizeof(buffer)) == (ssize_t)strlen("through")
			&& !memcmp(buffer, "through", strlen("through")));
	close(r.fds[0]);
	CHECK(read(pipe_fds[0], buffer, sizeof(buffer)) == 0);
	close(pipe_fds[0]);
	// A file and a directory, the file's description shared.
	int file = open("/tmp/datagram-passed", O_RDWR | O_CREAT | O_TRUNC, 0644);
	CHECK(file >= 0 && write(file, "contents", strlen("contents")) == (ssize_t)strlen("contents")
			&& lseek(file, 0, SEEK_SET) == 0);
	int dir = open("/tmp", O_RDONLY | O_DIRECTORY);
	CHECK(dir >= 0);
	int both[2] = {file, dir};
	CHECK(send_message(p[0], NULL, "f", 1, both, 2, NULL) == 1);
	receive(p[1], buffer, sizeof(buffer), CMSG_SPACE(2 * sizeof(int)), MSG_CMSG_CLOEXEC, &r);
	CHECK(r.got == 1 && r.count == 2 && !(r.flags & MSG_CTRUNC));
	CHECK(fcntl(r.fds[0], F_GETFD) & FD_CLOEXEC);
	CHECK(read(r.fds[0], buffer, strlen("con")) == (ssize_t)strlen("con")
			&& !memcmp(buffer, "con", strlen("con")));
	CHECK(lseek(file, 0, SEEK_CUR) == (off_t)strlen("con"));
	struct stat st;
	CHECK(fstat(r.fds[1], &st) == 0 && S_ISDIR(st.st_mode));
	close(r.fds[0]);
	close(r.fds[1]);
	close(dir);
	// A peek gets copies, and the receive that takes the datagram gets
	// its own.
	CHECK(send_message(p[0], NULL, "k", 1, &file, 1, NULL) == 1);
	receive(p[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), MSG_PEEK, &r);
	CHECK(r.got == 1 && r.count == 1);
	int peeked = r.fds[0];
	CHECK(lseek(peeked, 0, SEEK_CUR) == (off_t)strlen("con"));
	receive(p[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && r.count == 1 && r.fds[0] != peeked);
	CHECK(lseek(r.fds[0], 0, SEEK_CUR) == (off_t)strlen("con"));
	close(r.fds[0]);
	close(peeked);
	receive(p[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), MSG_DONTWAIT, &r);
	CHECK(r.got == -1 && r.error == EAGAIN);
	// Room for one of two: the other is closed, and MSG_CTRUNC says so.
	CHECK(pipe(pipe_fds) == 0);
	int two[2] = {file, pipe_fds[1]};
	CHECK(send_message(p[0], NULL, "t", 1, two, 2, NULL) == 1);
	close(pipe_fds[1]);
	receive(p[1], buffer, sizeof(buffer), CMSG_LEN(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && r.count == 1 && (r.flags & MSG_CTRUNC));
	close(r.fds[0]);
	CHECK(read(pipe_fds[0], buffer, sizeof(buffer)) == 0);
	close(pipe_fds[0]);
	// A datagram that is not received takes its descriptors with it.
	CHECK(pipe(pipe_fds) == 0);
	CHECK(send_message(p[0], NULL, "u", 1, &pipe_fds[1], 1, NULL) == 1);
	close(pipe_fds[1]);
	close(p[1]);
	CHECK(read(pipe_fds[0], buffer, sizeof(buffer)) == 0);
	close(pipe_fds[0]);
	close(p[0]);
	// A plain receive closes them, and the bytes come all the same.
	CHECK(socketpair(AF_UNIX, SOCK_DGRAM, 0, p) == 0);
	CHECK(pipe(pipe_fds) == 0);
	CHECK(send_message(p[0], NULL, "r", 1, &pipe_fds[1], 1, NULL) == 1);
	close(pipe_fds[1]);
	CHECK(read(p[1], buffer, sizeof(buffer)) == 1 && buffer[0] == 'r');
	CHECK(read(pipe_fds[0], buffer, sizeof(buffer)) == 0);
	close(pipe_fds[0]);
	// A datagram of descriptors alone has no bytes.
	CHECK(send_message(p[0], NULL, "", 0, &file, 1, NULL) == 0);
	receive(p[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), MSG_DONTWAIT, &r);
	CHECK(r.got == 0 && r.count == 1);
	close(r.fds[0]);
	// A descriptor that is none is refused, and nothing is sent.
	int bad = 1000;
	CHECK_ERROR(send_message(p[0], NULL, "b", 1, &bad, 1, NULL), EBADF);
	CHECK_ERROR(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	close(p[0]);
	close(p[1]);
	// To a name, descriptors go too.
	unlink(NAME);
	int receiver = datagram(NAME);
	int sender = datagram(NULL);
	CHECK(send_message(sender, NAME, "n", 1, &file, 1, NULL) == 1);
	receive(receiver, buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && r.count == 1 && r.from_len == 0);
	CHECK(lseek(r.fds[0], 0, SEEK_CUR) == (off_t)strlen("con"));
	close(r.fds[0]);
	close(sender);
	close(receiver);
	unlink(NAME);
	close(file);
	unlink("/tmp/datagram-passed");
}

// SO_PASSCRED and what it adds to a receive, on datagram sockets and on
// stream sockets; and what a process may send as its credentials.
static void identity(void) {
	int p[2];
	CHECK(socketpair(AF_UNIX, SOCK_DGRAM, 0, p) == 0);
	int value = -1;
	socklen_t size = sizeof(value);
	CHECK(getsockopt(p[1], SOL_SOCKET, SO_PASSCRED, &value, &size) == 0 && value == 0);
	int on = 5;
	CHECK_ERROR(setsockopt(p[1], SOL_SOCKET, SO_PASSCRED, &on, 1), EINVAL);
	CHECK(setsockopt(p[1], SOL_SOCKET, SO_PASSCRED, &on, sizeof(on)) == 0);
	size = sizeof(value);
	CHECK(getsockopt(p[1], SOL_SOCKET, SO_PASSCRED, &value, &size) == 0 && value == 1);
	// A socket gets credentials only when it asks for them.
	char buffer[16];
	struct received r;
	CHECK(sent(p[1], "x"));
	receive(p[0], buffer, sizeof(buffer), CMSG_SPACE(sizeof(struct ucred)), 0, &r);
	CHECK(r.got == 1 && !r.has_credentials && r.control_len == 0 && !(r.flags & MSG_CTRUNC));
	// Asked for, each datagram has the process that sent it, and its user
	// and group, in one message.
	CHECK(sent(p[0], "x"));
	receive(p[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(struct ucred)), 0, &r);
	CHECK(r.got == 1 && r.has_credentials && !(r.flags & MSG_CTRUNC));
	CHECK(r.credentials.pid == getpid() && r.credentials.uid == getuid()
			&& r.credentials.gid == getgid());
	CHECK(r.control_len >= CMSG_LEN(sizeof(struct ucred)));
	// A buffer with no room for the message loses it, and says so; so
	// does none at all.
	CHECK(sent(p[0], "x"));
	receive(p[1], buffer, sizeof(buffer), CMSG_LEN(sizeof(struct ucred)) - 1, 0, &r);
	CHECK(r.got == 1 && !r.has_credentials && (r.flags & MSG_CTRUNC));
	CHECK(sent(p[0], "x"));
	receive(p[1], buffer, sizeof(buffer), 0, 0, &r);
	CHECK(r.got == 1 && (r.flags & MSG_CTRUNC));
	// A datagram of another process has that process's, as at the send.
	pid_t pid = check_fork();
	if(pid == 0) {
		CHECK(sent(p[0], "c"));
		check_exit_child();
	}
	CHECK_CHILD(pid);
	receive(p[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(struct ucred)), 0, &r);
	CHECK(r.got == 1 && r.has_credentials && r.credentials.pid == pid);
	CHECK(r.credentials.uid == getuid() && r.credentials.gid == getgid());
	// Descriptors come after them, as on Linux, when the buffer has room
	// for both; with room for the credentials only, the descriptors are
	// closed, and MSG_CTRUNC says so.
	int file = open("/tmp/datagram-passed", O_RDWR | O_CREAT | O_TRUNC, 0644);
	CHECK(file >= 0);
	CHECK(send_message(p[0], NULL, "d", 1, &file, 1, NULL) == 1);
	receive(p[1], buffer, sizeof(buffer),
			CMSG_SPACE(sizeof(struct ucred)) + CMSG_SPACE(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && r.count == 1 && r.has_credentials && !(r.flags & MSG_CTRUNC));
	CHECK(r.first == SCM_CREDENTIALS);
	CHECK(r.control_len == CMSG_SPACE(sizeof(struct ucred)) + CMSG_SPACE(sizeof(int)));
	close(r.fds[0]);
	int pipe_fds[2];
	CHECK(pipe(pipe_fds) == 0);
	CHECK(send_message(p[0], NULL, "d", 1, &pipe_fds[1], 1, NULL) == 1);
	close(pipe_fds[1]);
	receive(p[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(struct ucred)), 0, &r);
	CHECK(r.got == 1 && r.count == 0 && r.has_credentials && (r.flags & MSG_CTRUNC));
	CHECK(read(pipe_fds[0], buffer, sizeof(buffer)) == 0);
	close(pipe_fds[0]);
	close(file);
	unlink("/tmp/datagram-passed");
	// A process may send its own credentials, which the receiver gets, and
	// nobody else's.
	struct ucred mine = {.pid = getpid(), .uid = getuid(), .gid = getgid()};
	CHECK(send_message(p[0], NULL, "m", 1, NULL, 0, &mine) == 1);
	receive(p[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(struct ucred)), 0, &r);
	CHECK(r.got == 1 && r.has_credentials && r.credentials.pid == mine.pid);
	CHECK(r.credentials.uid == mine.uid && r.credentials.gid == mine.gid);
	struct ucred theirs = mine;
	theirs.pid = mine.pid + 1;
	CHECK_ERROR(send_message(p[0], NULL, "m", 1, NULL, 0, &theirs), EPERM);
	theirs = mine;
	theirs.uid = mine.uid + 1;
	CHECK_ERROR(send_message(p[0], NULL, "m", 1, NULL, 0, &theirs), EPERM);
	theirs = mine;
	theirs.gid = mine.gid + 1;
	CHECK_ERROR(send_message(p[0], NULL, "m", 1, NULL, 0, &theirs), EPERM);
	CHECK_ERROR(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	// A message of another length, or of a kind a Unix socket does not
	// take, is invalid.
	union {
		struct cmsghdr header;
		char room[CMSG_SPACE(sizeof(int))];
	} control;
	memset(&control, 0, sizeof(control));
	struct iovec iov = {"s", 1};
	struct msghdr bad = {.msg_iov = &iov, .msg_iovlen = 1, .msg_control = control.room,
			.msg_controllen = sizeof(control.room)};
	struct cmsghdr *cmsg = CMSG_FIRSTHDR(&bad);
	cmsg->cmsg_level = SOL_SOCKET;
	cmsg->cmsg_type = SCM_CREDENTIALS;
	cmsg->cmsg_len = CMSG_LEN(sizeof(int));
	CHECK_ERROR(sendmsg(p[0], &bad, 0), EINVAL);
	cmsg->cmsg_type = 99;
	CHECK_ERROR(sendmsg(p[0], &bad, 0), EINVAL);
	CHECK_ERROR(recv(p[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	// The peer of a pair is the process that made it, and stays so when the
	// socket forgets its peer.
	struct sockaddr unspec = {.sa_family = AF_UNSPEC};
	CHECK(peer_is(p[0], getpid()));
	CHECK(connect(p[0], &unspec, sizeof(unspec)) == 0);
	CHECK(peer_is(p[0], getpid()));
	close(p[0]);
	close(p[1]);
	// A socket with no peer to speak of has none for SO_PEERCRED either.
	int lone = datagram(NULL);
	CHECK(peer_unset(lone));
	close(lone);
	// By name, the credentials come with the datagram from a socket that
	// was not made with the receiver.
	unlink(NAME);
	int receiver = datagram(NAME);
	CHECK(setsockopt(receiver, SOL_SOCKET, SO_PASSCRED, &on, sizeof(on)) == 0);
	int sender = datagram(NULL);
	CHECK(sent_to(sender, NAME, "n"));
	receive(receiver, buffer, sizeof(buffer), CMSG_SPACE(sizeof(struct ucred)), 0, &r);
	CHECK(r.got == 1 && r.has_credentials && r.credentials.pid == getpid());
	CHECK(r.from_len == 0);
	CHECK(send_message(sender, NAME, "n", 1, NULL, 0, &mine) == 1);
	receive(receiver, buffer, sizeof(buffer), CMSG_SPACE(sizeof(struct ucred)), 0, &r);
	CHECK(r.got == 1 && r.has_credentials && r.credentials.pid == mine.pid);
	close(sender);
	close(receiver);
	unlink(NAME);
	// A stream's receive has the connection's peer, as SO_PEERCRED does.
	int s[2];
	CHECK(socketpair(AF_UNIX, SOCK_STREAM, 0, s) == 0);
	size = sizeof(value);
	CHECK(getsockopt(s[1], SOL_SOCKET, SO_PASSCRED, &value, &size) == 0 && value == 0);
	CHECK(setsockopt(s[1], SOL_SOCKET, SO_PASSCRED, &on, sizeof(on)) == 0);
	size = sizeof(value);
	CHECK(getsockopt(s[1], SOL_SOCKET, SO_PASSCRED, &value, &size) == 0 && value == 1);
	CHECK(write(s[0], "data", strlen("data")) == (ssize_t)strlen("data"));
	receive(s[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(struct ucred)), 0, &r);
	CHECK(r.got == (ssize_t)strlen("data") && r.has_credentials && r.credentials.pid == getpid());
	CHECK(r.credentials.uid == getuid() && r.credentials.gid == getgid());
	CHECK(send_message(s[0], NULL, "m", 1, NULL, 0, &mine) == 1);
	theirs = mine;
	theirs.pid = mine.pid + 1;
	CHECK_ERROR(send_message(s[0], NULL, "m", 1, NULL, 0, &theirs), EPERM);
	close(s[0]);
	close(s[1]);
	// A connection to a named socket has the peers' processes: the
	// accepted end's is the one that connected, the connecting end's the
	// one that listened.
	unlink(STREAM);
	int listener = socket(AF_UNIX, SOCK_STREAM, 0);
	struct sockaddr_un addr;
	socklen_t len = address_of(STREAM, &addr);
	CHECK(bind(listener, (struct sockaddr *)&addr, len) == 0);
	CHECK(listen(listener, 1) == 0);
	pid = check_fork();
	if(pid == 0) {
		alarm(PATIENCE);
		int c = socket(AF_UNIX, SOCK_STREAM, 0);
		CHECK(connect(c, (struct sockaddr *)&addr, len) == 0);
		CHECK(peer_is(c, getppid()));
		CHECK(write(c, "hi", strlen("hi")) == (ssize_t)strlen("hi"));
		// The accepted end closes when it has seen who came.
		CHECK(read(c, buffer, sizeof(buffer)) == 0);
		check_exit_child();
	}
	int accepted = accept(listener, NULL, NULL);
	CHECK(accepted >= 0);
	CHECK(setsockopt(accepted, SOL_SOCKET, SO_PASSCRED, &on, sizeof(on)) == 0);
	CHECK(peer_is(accepted, pid));
	receive(accepted, buffer, sizeof(buffer), CMSG_SPACE(sizeof(struct ucred)), 0, &r);
	CHECK(r.got == (ssize_t)strlen("hi") && r.has_credentials && r.credentials.pid == pid);
	close(accepted);
	CHECK_CHILD(pid);
	close(listener);
	unlink(STREAM);
}

// Makes a pair of sockets of type `type`.
static void pair_of(int type, int *p) {
	CHECK(socketpair(AF_UNIX, type, 0, p) == 0);
}

static void sockets(void) {
	int d[2];
	pair_of(SOCK_DGRAM, d);
	char buffer[16];
	struct received r;
	// A stream socket's end, in a datagram: the end that arrives talks to
	// the end that stayed, which ends the stream when it is closed.
	int s[2];
	pair_of(SOCK_STREAM, s);
	CHECK(send_message(d[0], NULL, "s", 1, &s[1], 1, NULL) == 1);
	close(s[1]);
	receive(d[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && r.count == 1 && !(r.flags & MSG_CTRUNC));
	int got = r.fds[0];
	struct stat st;
	CHECK(fstat(got, &st) == 0 && S_ISSOCK(st.st_mode));
	int value = -1;
	socklen_t size = sizeof(value);
	CHECK(getsockopt(got, SOL_SOCKET, SO_TYPE, &value, &size) == 0 && value == SOCK_STREAM);
	CHECK(named_as(got, 0, NULL));
	CHECK(write(s[0], "through", strlen("through")) == (ssize_t)strlen("through"));
	CHECK(read(got, buffer, sizeof(buffer)) == (ssize_t)strlen("through")
			&& !memcmp(buffer, "through", strlen("through")));
	CHECK(write(got, "back", strlen("back")) == (ssize_t)strlen("back"));
	CHECK(read(s[0], buffer, sizeof(buffer)) == (ssize_t)strlen("back")
			&& !memcmp(buffer, "back", strlen("back")));
	close(s[0]);
	CHECK(read(got, buffer, sizeof(buffer)) == 0);
	close(got);
	// A named datagram socket, in a stream: the name stays with the socket
	// that is sent, though the sender closes its own.
	unlink(NAME);
	int bound = datagram(NAME);
	pair_of(SOCK_STREAM, s);
	CHECK(send_message(s[0], NULL, "d", 1, &bound, 1, NULL) == 1);
	close(bound);
	receive(s[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && r.count == 1);
	got = r.fds[0];
	CHECK(named_as(got, 0, NAME));
	size = sizeof(value);
	CHECK(getsockopt(got, SOL_SOCKET, SO_TYPE, &value, &size) == 0 && value == SOCK_DGRAM);
	int anon = datagram(NULL);
	CHECK(sent_to(anon, NAME, "to the copy"));
	CHECK(received(got, 0, "to the copy"));
	close(anon);
	close(got);
	close(s[0]);
	close(s[1]);
	unlink(NAME);
	// A datagram pair's end, in a datagram.
	int e[2];
	pair_of(SOCK_DGRAM, e);
	CHECK(send_message(d[0], NULL, "e", 1, &e[1], 1, NULL) == 1);
	close(e[1]);
	receive(d[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && r.count == 1);
	got = r.fds[0];
	CHECK(sent(e[0], "pairs"));
	CHECK(received(got, 0, "pairs"));
	close(got);
	close(e[0]);
	// A listening socket: a connection is accepted through the copy, after
	// the sender has closed its own.
	unlink(STREAM);
	int listener = socket(AF_UNIX, SOCK_STREAM, 0);
	struct sockaddr_un addr;
	socklen_t len = address_of(STREAM, &addr);
	CHECK(bind(listener, (struct sockaddr *)&addr, len) == 0);
	CHECK(listen(listener, 2) == 0);
	CHECK(send_message(d[0], NULL, "l", 1, &listener, 1, NULL) == 1);
	close(listener);
	receive(d[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && r.count == 1);
	got = r.fds[0];
	size = sizeof(value);
	CHECK(getsockopt(got, SOL_SOCKET, SO_ACCEPTCONN, &value, &size) == 0 && value == 1);
	int client = socket(AF_UNIX, SOCK_STREAM, 0);
	CHECK(connect(client, (struct sockaddr *)&addr, len) == 0);
	int accepted = accept(got, NULL, NULL);
	CHECK(accepted >= 0);
	CHECK(write(client, "hello", strlen("hello")) == (ssize_t)strlen("hello"));
	CHECK(read(accepted, buffer, sizeof(buffer)) == (ssize_t)strlen("hello")
			&& !memcmp(buffer, "hello", strlen("hello")));
	close(accepted);
	close(client);
	close(got);
	unlink(STREAM);
	// A socket may go over the connection it is the end of, into the queue
	// of the other end, which it does not read.
	pair_of(SOCK_STREAM, s);
	CHECK(send_message(s[0], NULL, "o", 1, &s[0], 1, NULL) == 1);
	receive(s[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && r.count == 1);
	CHECK(write(r.fds[0], "z", 1) == 1);
	CHECK(read(s[1], buffer, sizeof(buffer)) == 1 && buffer[0] == 'z');
	close(r.fds[0]);
	// The other end may not go over it: it would be in the queue it reads.
	CHECK_ERROR(send_message(s[0], NULL, "x", 1, &s[1], 1, NULL), ETOOMANYREFS);
	CHECK_ERROR(recv(s[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	close(s[0]);
	close(s[1]);
	pair_of(SOCK_DGRAM, e);
	CHECK(send_message(e[0], NULL, "o", 1, &e[0], 1, NULL) == 1);
	receive(e[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && r.count == 1);
	close(r.fds[0]);
	CHECK_ERROR(send_message(e[0], NULL, "x", 1, &e[1], 1, NULL), ETOOMANYREFS);
	CHECK_ERROR(recv(e[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	close(e[0]);
	close(e[1]);
	// A socket may not be sent to itself either, by name.
	unlink(NAME);
	bound = datagram(NAME);
	CHECK_ERROR(send_message(bound, NAME, "x", 1, &bound, 1, NULL), ETOOMANYREFS);
	close(bound);
	unlink(NAME);
	// A socket whose queue holds a socket that was not received may not be
	// sent, whichever kind either is, until the socket in it is received.
	int x[2];
	int y[2];
	pair_of(SOCK_DGRAM, x);
	pair_of(SOCK_STREAM, y);
	CHECK(send_message(x[0], NULL, "1", 1, &y[0], 1, NULL) == 1);
	CHECK_ERROR(send_message(d[0], NULL, "2", 1, &x[1], 1, NULL), ETOOMANYREFS);
	receive(x[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && r.count == 1);
	close(r.fds[0]);
	CHECK(send_message(d[0], NULL, "2", 1, &x[1], 1, NULL) == 1);
	receive(d[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && r.count == 1);
	close(r.fds[0]);
	close(x[0]);
	close(x[1]);
	int z[2];
	pair_of(SOCK_STREAM, z);
	CHECK(send_message(z[0], NULL, "1", 1, &y[0], 1, NULL) == 1);
	CHECK_ERROR(send_message(d[0], NULL, "2", 1, &z[1], 1, NULL), ETOOMANYREFS);
	CHECK_ERROR(recv(d[1], buffer, sizeof(buffer), MSG_DONTWAIT), EAGAIN);
	receive(z[1], buffer, sizeof(buffer), CMSG_SPACE(sizeof(int)), 0, &r);
	CHECK(r.got == 1 && r.count == 1);
	close(r.fds[0]);
	CHECK(send_message(d[0], NULL, "2", 1, &z[1], 1, NULL) == 1);
	close(z[0]);
	close(z[1]);
	close(y[0]);
	close(y[1]);
	close(d[0]);
	close(d[1]);
}

// How much memory is free, in KiB, or -1.
static long long free_kib(void) {
	static char text[8192];
	int fd = open("/processes/meminfo", O_RDONLY | O_CLOEXEC);
	if(fd < 0)
		return -1;
	ssize_t got = read(fd, text, sizeof(text) - 1);
	close(fd);
	if(got <= 0)
		return -1;
	text[got] = 0;
	const char *at = strstr(text, "\nfree: ");
	return at ? atoll(at + strlen("\nfree: ")) : -1;
}

// A pair of the `carrier` type, an end of a pair of the `passenger` type
// sent over it with bytes waiting in the passenger's, and everything
// closed while the end is in flight, the carrier's or the passenger's
// first.
static void round_trip(int carrier, int passenger, int carrier_first) {
	int c[2];
	int t[2];
	pair_of(carrier, c);
	pair_of(passenger, t);
	static const char block[100] = {1};
	CHECK(send(t[0], block, sizeof(block), 0) == (ssize_t)sizeof(block));
	CHECK(send_message(c[0], NULL, "s", 1, &t[1], 1, NULL) == 1);
	if(carrier_first) {
		close(c[0]);
		close(c[1]);
		close(t[0]);
		close(t[1]);
	} else {
		close(t[0]);
		close(t[1]);
		close(c[0]);
		close(c[1]);
	}
}

// Each pair's end in the queue of the next pair's, `depth` pairs, with
// data waiting in each: the first end is the only one a descriptor holds,
// and closing it frees them all, one after the other. Going backwards, the
// end that is sent has nothing in its own queue yet, as it must not.
static void chain(int depth) {
	static char data[4000];
	int head[2];
	pair_of(SOCK_DGRAM, head);
	int last = head[0];
	for(int i = 0; i < depth; i++) {
		int next[2];
		pair_of(SOCK_DGRAM, next);
		CHECK(send(next[0], data, sizeof(data), 0) == (ssize_t)sizeof(data));
		CHECK(send_message(last, NULL, "n", 1, &next[1], 1, NULL) == 1);
		close(next[1]);
		close(last);
		last = next[0];
	}
	close(last);
	close(head[1]);
}

// Sockets in flight and closed: the memory that goes with them comes back.
// And a chain of them, each in the next one's queue, which is freed when
// its end is closed, one socket after the other. The first rounds of each
// make what the kernel makes once and keeps.
static void leaking(void) {
	static const int kinds[2] = {SOCK_STREAM, SOCK_DGRAM};
	for(int i = 0; i < 30; i++)
		round_trip(kinds[i % 2], kinds[(i / 2) % 2], i % 3 == 0);
	long long before = free_kib();
	CHECK(before > 0);
	for(int i = 0; i < 1000; i++)
		round_trip(kinds[i % 2], kinds[(i / 2) % 2], (i / 4) % 2);
	long long after = free_kib();
	printf("%s: %lld KiB free before the rounds, %lld after\n", check_name, before, after);
	CHECK(after > 0 && before - after < 2048);
	enum { DEEP = 300 };
	chain(DEEP);
	before = free_kib();
	chain(DEEP);
	after = free_kib();
	printf("%s: %lld KiB free before the chain, %lld after\n", check_name, before, after);
	CHECK(after > 0 && before - after < 512);
}

int main(void) {
	check_start("datagram");
	PART(made);
	PART(pair);
	PART(vectors);
	PART(named);
	PART(limits);
	PART(shutting);
	PART(descriptors);
	PART(identity);
	PART(sockets);
	PART(leaking);
	return check_end();
}
