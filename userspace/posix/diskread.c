// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the io-weight boot test runs as services (ADR 0476): threads that
// read a disk raw, 1 MiB at a time, round and round its first SPAN_MIB
// MiB, so that every read goes to the disk: a block device's reads have
// nothing cached between (radix-block(2)). With enough threads there are
// more requests than the disk lets go at once, so its queue decides whose
// go first, by the weights of the services' domains, and their rates hold
// a domain back.
//
//   diskread DEVICE THREADS SPAN_MIB

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define CHUNK ((size_t)1 << 20)

static int disk;
// The MiB the threads read round and round.
static unsigned span;

// A thread reads from its own place in the span, a MiB after the last.
static void *reader(void *arg) {
	unsigned at = (unsigned)(uintptr_t)arg % span;
	char *buffer = malloc(CHUNK);
	if(!buffer) {
		printf("diskread: no memory for a buffer\n");
		fflush(stdout);
		exit(1);
	}
	for(;;) {
		ssize_t got = pread(disk, buffer, CHUNK, (off_t)at * (off_t)CHUNK);
		if(got != (ssize_t)CHUNK) {
			printf("diskread: a read of 1 MiB at %u MiB gave %zd: %s\n", at, got, strerror(errno));
			fflush(stdout);
			exit(1);
		}
		at = (at + 1) % span;
	}
	return NULL;
}

int main(int argc, char **argv) {
	if(argc != 4) {
		printf("usage: diskread DEVICE THREADS SPAN_MIB\n");
		return 2;
	}
	unsigned threads = (unsigned)atoi(argv[2]);
	span = (unsigned)atoi(argv[3]);
	if(!threads || !span) {
		printf("diskread: THREADS and SPAN_MIB are numbers of 1 or more\n");
		return 2;
	}
	disk = open(argv[1], O_RDONLY | O_CLOEXEC);
	if(disk < 0) {
		printf("diskread: %s: %s\n", argv[1], strerror(errno));
		return 1;
	}
	printf("diskread: %u threads read %u MiB of %s\n", threads, span, argv[1]);
	fflush(stdout);
	for(unsigned n = 0; n < threads; n++) {
		pthread_t thread;
		int err = pthread_create(&thread, NULL, reader, (void *)(uintptr_t)(n * (span / threads + 1)));
		if(err) {
			printf("diskread: thread %u: %s\n", n, strerror(err));
			fflush(stdout);
			return 1;
		}
	}
	// The threads read until the service is stopped.
	for(;;)
		pause();
}
