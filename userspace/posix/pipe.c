// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Pipes: data in order, the end of the stream, SIGPIPE and EPIPE, a pipe
// that fills, non-blocking ends, atomic writes, descriptors made with
// dup, dup2, dup3 and fcntl, and the size of a pipe.

#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/stat.h>

#include "check.h"

static void basics(void) {
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(fds[0] >= 0 && fds[1] >= 0 && fds[0] != fds[1]);
	CHECK(write(fds[1], "abc", 3) == 3);
	CHECK(write(fds[1], "def", 3) == 3);
	char buffer[16] = {0};
	CHECK(read(fds[0], buffer, 2) == 2 && !memcmp(buffer, "ab", 2));
	CHECK(read(fds[0], buffer, sizeof(buffer)) == 4 && !memcmp(buffer, "cdef", 4));
	// The reading end cannot be written, nor the writing end read.
	CHECK_ERROR(write(fds[0], "x", 1), EBADF);
	CHECK_ERROR(read(fds[1], buffer, 1), EBADF);
	CHECK_ERROR(lseek(fds[0], 0, SEEK_SET), ESPIPE);
	int readable = -1;
	CHECK(write(fds[1], "12345", 5) == 5);
	CHECK(ioctl(fds[0], FIONREAD, &readable) == 0 && readable == 5);
	CHECK(read(fds[0], buffer, sizeof(buffer)) == 5);
	// The end of the stream, once no writer is left.
	close(fds[1]);
	CHECK(read(fds[0], buffer, sizeof(buffer)) == 0);
	close(fds[0]);
}

static void broken(void) {
	// Writing with no reader left: SIGPIPE, which ends the process.
	int fds[2];
	CHECK(pipe(fds) == 0);
	close(fds[0]);
	pid_t pid = check_fork();
	if(pid == 0) {
		write(fds[1], "x", 1);
		_exit(0);
	}
	int status = check_wait(pid);
	CHECK(WIFSIGNALED(status) && WTERMSIG(status) == SIGPIPE);
	// With SIGPIPE ignored, EPIPE.
	signal(SIGPIPE, SIG_IGN);
	CHECK_ERROR(write(fds[1], "x", 1), EPIPE);
	signal(SIGPIPE, SIG_DFL);
	close(fds[1]);
}

// A megabyte through a pipe, which fills and waits for the reader.
static void streams(void) {
	int fds[2];
	CHECK(pipe(fds) == 0);
	enum { TOTAL = 1 << 20 };
	pid_t pid = check_fork();
	if(pid == 0) {
		close(fds[0]);
		unsigned char chunk[3001];
		size_t sent = 0;
		while(sent < TOTAL) {
			size_t n = TOTAL - sent < sizeof(chunk) ? TOTAL - sent : sizeof(chunk);
			for(size_t i = 0; i < n; i++)
				chunk[i] = (unsigned char)((sent + i) % 251);
			ssize_t wrote = write(fds[1], chunk, n);
			if(wrote <= 0)
				_exit(1);
			sent += (size_t)wrote;
		}
		_exit(0);
	}
	close(fds[1]);
	unsigned char buffer[4096];
	size_t got = 0, wrong = 0;
	ssize_t n;
	while((n = read(fds[0], buffer, sizeof(buffer))) > 0) {
		for(ssize_t i = 0; i < n; i++)
			wrong += buffer[i] != (unsigned char)((got + (size_t)i) % 251);
		got += (size_t)n;
	}
	CHECK(n == 0 && got == TOTAL && wrong == 0);
	CHECK_CHILD(pid);
	close(fds[0]);
}

static void nonblocking(void) {
	int fds[2];
	CHECK(pipe2(fds, O_NONBLOCK) == 0);
	CHECK(fcntl(fds[0], F_GETFL) & O_NONBLOCK);
	char buffer[4096];
	CHECK_ERROR(read(fds[0], buffer, 1), EAGAIN);
	// Fill it up: then a write would wait.
	memset(buffer, 'z', sizeof(buffer));
	size_t room = 0;
	ssize_t n;
	while((n = write(fds[1], buffer, sizeof(buffer))) > 0)
		room += (size_t)n;
	CHECK(n == -1 && errno == EAGAIN);
	CHECK(room >= 4096);
	// A write of PIPE_BUF or less goes in whole or not at all.
	CHECK(read(fds[0], buffer, 100) == 100);
	CHECK_ERROR(write(fds[1], buffer, PIPE_BUF), EAGAIN);
	// Back to blocking, with fcntl.
	int flags = fcntl(fds[0], F_GETFL);
	CHECK(fcntl(fds[0], F_SETFL, flags & ~O_NONBLOCK) == 0);
	CHECK(!(fcntl(fds[0], F_GETFL) & O_NONBLOCK));
	CHECK((fcntl(fds[0], F_GETFL) & O_ACCMODE) == O_RDONLY);
	CHECK((fcntl(fds[1], F_GETFL) & O_ACCMODE) == O_WRONLY);
	close(fds[0]);
	close(fds[1]);
}

// Writes of PIPE_BUF bytes or fewer from two writers never interleave.
static void atomic(void) {
	int fds[2];
	CHECK(pipe(fds) == 0);
	enum { MESSAGES = 300, SIZE = 512 };
	pid_t writers[2];
	for(int w = 0; w < 2; w++) {
		writers[w] = check_fork();
		if(writers[w] == 0) {
			close(fds[0]);
			char message[SIZE];
			memset(message, 'a' + w, sizeof(message));
			for(int i = 0; i < MESSAGES; i++) {
				if(write(fds[1], message, sizeof(message)) != SIZE)
					_exit(1);
			}
			_exit(0);
		}
	}
	close(fds[1]);
	char message[SIZE];
	int counts[2] = {0, 0}, torn = 0;
	for(;;) {
		size_t got = 0;
		ssize_t n = 0;
		while(got < SIZE && (n = read(fds[0], message + got, SIZE - got)) > 0)
			got += (size_t)n;
		if(got == 0)
			break;
		int whole = got == SIZE;
		for(size_t i = 1; i < got && whole; i++)
			whole = message[i] == message[0];
		if(whole && (message[0] == 'a' || message[0] == 'b'))
			counts[message[0] - 'a']++;
		else
			torn++;
	}
	CHECK(torn == 0 && counts[0] == MESSAGES && counts[1] == MESSAGES);
	CHECK_CHILD(writers[0]);
	CHECK_CHILD(writers[1]);
	close(fds[0]);
}

static void duplicates(void) {
	int fds[2];
	CHECK(pipe(fds) == 0);
	int copy = dup(fds[1]);
	CHECK(copy >= 0);
	close(fds[1]);
	// The copy keeps the pipe open.
	CHECK(write(copy, "d", 1) == 1);
	CHECK(dup2(copy, 50) == 50);
	CHECK(write(50, "e", 1) == 1);
	CHECK(dup2(50, 50) == 50);
	CHECK_ERROR(dup2(50, -1), EBADF);
	CHECK_ERROR(dup(1000), EBADF);
	CHECK(dup3(copy, 51, O_CLOEXEC) == 51 && fcntl(51, F_GETFD) == FD_CLOEXEC);
	CHECK_ERROR(dup3(51, 51, 0), EINVAL);
	// F_DUPFD takes the lowest free descriptor from its argument on.
	int high = fcntl(copy, F_DUPFD, 60);
	CHECK(high == 60);
	int high_cloexec = fcntl(copy, F_DUPFD_CLOEXEC, 60);
	CHECK(high_cloexec == 61 && fcntl(high_cloexec, F_GETFD) == FD_CLOEXEC);
	char buffer[4] = {0};
	CHECK(read(fds[0], buffer, sizeof(buffer)) == 2 && !memcmp(buffer, "de", 2));
	close(copy);
	close(50);
	close(51);
	close(high);
	close(high_cloexec);
	// The lowest free descriptor comes first.
	int lowest = dup(fds[0]);
	CHECK(lowest == fds[1]);
	close(lowest);
	close(fds[0]);
	CHECK_ERROR(close(fds[0]), EBADF);
}

// A pipeline of three processes, as a shell makes one.
static void pipeline(void) {
	int first[2], second[2];
	CHECK(pipe(first) == 0 && pipe(second) == 0);
	pid_t producer = check_fork();
	if(producer == 0) {
		dup2(first[1], STDOUT_FILENO);
		close(first[0]);
		close(first[1]);
		close(second[0]);
		close(second[1]);
		for(int i = 0; i < 100; i++)
			printf("line %d\n", i);
		exit(0);
	}
	pid_t filter = check_fork();
	if(filter == 0) {
		dup2(first[0], STDIN_FILENO);
		dup2(second[1], STDOUT_FILENO);
		close(first[0]);
		close(first[1]);
		close(second[0]);
		close(second[1]);
		char line[64];
		int count = 0;
		while(fgets(line, sizeof(line), stdin))
			count++;
		printf("%d\n", count);
		exit(0);
	}
	close(first[0]);
	close(first[1]);
	close(second[1]);
	char buffer[16] = {0};
	ssize_t got = 0, n;
	while((n = read(second[0], buffer + got, sizeof(buffer) - 1 - (size_t)got)) > 0)
		got += n;
	CHECK(!strcmp(buffer, "100\n"));
	close(second[0]);
	CHECK_CHILD(producer);
	CHECK_CHILD(filter);
}

// The size of a pipe, as F_GETPIPE_SZ and F_SETPIPE_SZ have it.
static void sizes(void) {
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(fcntl(fds[0], F_GETPIPE_SZ) == 65536 && fcntl(fds[1], F_GETPIPE_SZ) == 65536);
	// A size goes up to a power of two, and no lower than a page.
	CHECK(fcntl(fds[1], F_SETPIPE_SZ, 5000) == 8192);
	CHECK(fcntl(fds[0], F_GETPIPE_SZ) == 8192 && fcntl(fds[1], F_GETPIPE_SZ) == 8192);
	CHECK(fcntl(fds[1], F_SETPIPE_SZ, 1) == 4096);
	CHECK(fcntl(fds[1], F_SETPIPE_SZ, 0) == 4096);
	CHECK_ERROR(fcntl(fds[1], F_SETPIPE_SZ, -1), EINVAL);
	CHECK(fcntl(fds[0], F_GETPIPE_SZ) == 4096);
	// A pipe grows to a megabyte at most; 1 << 31 is the largest size
	// asked for that is not invalid.
	CHECK_ERROR(fcntl(fds[1], F_SETPIPE_SZ, 1u << 31), EPERM);
	CHECK(fcntl(fds[1], F_SETPIPE_SZ, 1048576) == 1048576);
	CHECK_ERROR(fcntl(fds[1], F_SETPIPE_SZ, 2097152), EPERM);
	CHECK(fcntl(fds[0], F_GETPIPE_SZ) == 1048576);
	CHECK(fcntl(fds[1], F_SETPIPE_SZ, 65536) == 65536);
	close(fds[0]);
	close(fds[1]);

	// What a pipe holds comes out in order after it is resized, even when
	// it wrapped around the end of its buffer.
	CHECK(pipe(fds) == 0);
	CHECK(fcntl(fds[1], F_SETPIPE_SZ, 8192) == 8192);
	unsigned char pattern[10000], buffer[10000];
	for(size_t i = 0; i < sizeof(pattern); i++)
		pattern[i] = (unsigned char)(i % 251);
	CHECK(write(fds[1], pattern, 6000) == 6000);
	CHECK(read(fds[0], buffer, 4000) == 4000 && !memcmp(buffer, pattern, 4000));
	CHECK(write(fds[1], pattern + 6000, 4000) == 4000);
	CHECK(fcntl(fds[1], F_SETPIPE_SZ, 65536) == 65536);
	CHECK(read(fds[0], buffer, sizeof(buffer)) == 6000);
	CHECK(!memcmp(buffer, pattern + 4000, 6000));
	// A pipe does not shrink below what it holds.
	CHECK(fcntl(fds[1], F_SETPIPE_SZ, 8192) == 8192);
	CHECK(write(fds[1], pattern, 6000) == 6000);
	CHECK_ERROR(fcntl(fds[1], F_SETPIPE_SZ, 4096), EBUSY);
	CHECK(fcntl(fds[1], F_GETPIPE_SZ) == 8192);
	CHECK(read(fds[0], buffer, sizeof(buffer)) == 6000 && !memcmp(buffer, pattern, 6000));
	close(fds[0]);
	close(fds[1]);

	// A larger pipe has room for more, and polling says so.
	CHECK(pipe2(fds, O_NONBLOCK) == 0);
	CHECK(fcntl(fds[1], F_SETPIPE_SZ, 4096) == 4096);
	CHECK(write(fds[1], pattern, 4096) == 4096);
	CHECK_ERROR(write(fds[1], pattern, 1), EAGAIN);
	struct pollfd room = {fds[1], POLLOUT, 0};
	CHECK(poll(&room, 1, 0) == 0);
	CHECK(fcntl(fds[1], F_SETPIPE_SZ, 8192) == 8192);
	room.revents = 0;
	CHECK(poll(&room, 1, 0) == 1 && (room.revents & POLLOUT));
	CHECK(write(fds[1], pattern, 4096) == 4096);
	close(fds[0]);
	close(fds[1]);

	// Only pipes have a size.
	int file = open("/tmp/pipe-sizes", O_RDWR | O_CREAT | O_TRUNC, 0600);
	CHECK(file >= 0);
	CHECK_ERROR(fcntl(file, F_GETPIPE_SZ), EBADF);
	CHECK_ERROR(fcntl(file, F_SETPIPE_SZ, 4096), EBADF);
	close(file);
	unlink("/tmp/pipe-sizes");
	int pair[2];
	CHECK(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
	CHECK_ERROR(fcntl(pair[0], F_GETPIPE_SZ), EBADF);
	close(pair[0]);
	close(pair[1]);

	// A named pipe has one too.
	unlink("/tmp/pipe-sizes-fifo");
	CHECK(mkfifo("/tmp/pipe-sizes-fifo", 0600) == 0);
	int fifo = open("/tmp/pipe-sizes-fifo", O_RDWR | O_NONBLOCK);
	CHECK(fifo >= 0);
	CHECK(fcntl(fifo, F_GETPIPE_SZ) == 65536);
	CHECK(fcntl(fifo, F_SETPIPE_SZ, 16384) == 16384);
	close(fifo);
	unlink("/tmp/pipe-sizes-fifo");
}

int main(void) {
	check_start("pipe");
	RUN(basics);
	RUN(broken);
	RUN(streams);
	RUN(nonblocking);
	RUN(atomic);
	RUN(duplicates);
	RUN(pipeline);
	RUN(sizes);
	return check_end();
}
