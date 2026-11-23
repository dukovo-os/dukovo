// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Memory files, memfd_create(2) (ADR 0614): what a file made unlinked
// reads and writes, its flags and name, a shared mapping of it that every
// process that has it sees, and its seals: each one's refusal, EBUSY for a
// write seal while a writable shared mapping exists (in this process or
// another), and the seal that seals the seals.

#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <fcntl.h>
#include <stdint.h>
#include <sys/mman.h>
#include <sys/stat.h>

#include "check.h"

#ifndef MFD_CLOEXEC
#define MFD_CLOEXEC 0x1U
#endif
#ifndef MFD_ALLOW_SEALING
#define MFD_ALLOW_SEALING 0x2U
#endif
#ifndef MFD_NOEXEC_SEAL
#define MFD_NOEXEC_SEAL 0x8U
#endif
#ifndef MFD_EXEC
#define MFD_EXEC 0x10U
#endif
#ifndef F_ADD_SEALS
#define F_ADD_SEALS 1033
#define F_GET_SEALS 1034
#endif
#ifndef F_SEAL_SEAL
#define F_SEAL_SEAL 0x1
#define F_SEAL_SHRINK 0x2
#define F_SEAL_GROW 0x4
#define F_SEAL_WRITE 0x8
#endif
#ifndef F_SEAL_FUTURE_WRITE
#define F_SEAL_FUTURE_WRITE 0x10
#endif
#ifndef F_SEAL_EXEC
#define F_SEAL_EXEC 0x20
#endif

#define PAGE 4096

// A memory file that may be sealed, `pages` pages long.
static int sealable(size_t pages) {
	int fd = memfd_create("memfd-test", MFD_ALLOW_SEALING | MFD_CLOEXEC);
	CHECK(fd >= 0);
	CHECK(ftruncate(fd, (off_t)(pages * PAGE)) == 0);
	return fd;
}

// The seals of `fd`.
static int seals_of(int fd) {
	int seals = fcntl(fd, F_GET_SEALS);
	CHECK(seals >= 0);
	return seals;
}

// Maps `pages` pages of `fd` shared, with `prot`; MAP_FAILED if it fails.
static void *map_shared(int fd, size_t pages, int prot) {
	return mmap(NULL, pages * PAGE, prot, MAP_SHARED, fd, 0);
}

static void reads_and_writes(void) {
	int fd = memfd_create("memfd-test", 0);
	CHECK(fd >= 0);
	struct stat st;
	CHECK(fstat(fd, &st) == 0);
	// Empty, a regular file that no directory holds.
	CHECK(S_ISREG(st.st_mode) && st.st_size == 0 && st.st_nlink == 0);
	CHECK(write(fd, "hello", 5) == 5);
	CHECK(fstat(fd, &st) == 0 && st.st_size == 5);
	char buffer[16] = {0};
	CHECK(pread(fd, buffer, sizeof(buffer), 0) == 5 && !memcmp(buffer, "hello", 5));
	// The offset is the description's: the write moved it.
	CHECK(lseek(fd, 0, SEEK_CUR) == 5);
	CHECK(lseek(fd, 1, SEEK_SET) == 1);
	CHECK(read(fd, buffer, 2) == 2 && !memcmp(buffer, "el", 2));
	CHECK(read(fd, buffer, 16) == 2 && !memcmp(buffer, "lo", 2));
	CHECK(read(fd, buffer, 16) == 0);
	// Growing reads as zeros.
	CHECK(ftruncate(fd, 8) == 0);
	memset(buffer, 1, sizeof(buffer));
	CHECK(pread(fd, buffer, 3, 5) == 3 && !memcmp(buffer, "\0\0\0", 3));
	CHECK(ftruncate(fd, 2) == 0);
	CHECK(pread(fd, buffer, sizeof(buffer), 0) == 2 && !memcmp(buffer, "he", 2));
	// A copy of the descriptor is the same file at the same offset.
	int copy = dup(fd);
	CHECK(copy >= 0);
	CHECK(lseek(fd, 0, SEEK_SET) == 0);
	CHECK(read(copy, buffer, 1) == 1 && buffer[0] == 'h');
	close(copy);
	close(fd);
}

static void flags_and_names(void) {
	int plain = memfd_create("a", 0);
	CHECK(plain >= 0);
	CHECK((fcntl(plain, F_GETFD) & FD_CLOEXEC) == 0);
	int closing = memfd_create("a", MFD_CLOEXEC);
	CHECK(closing >= 0);
	CHECK((fcntl(closing, F_GETFD) & FD_CLOEXEC) != 0);
	// Open for reading and writing.
	CHECK((fcntl(plain, F_GETFL) & O_ACCMODE) == O_RDWR);
	// Two files of one name are two files.
	CHECK(write(plain, "x", 1) == 1);
	CHECK(lseek(closing, 0, SEEK_END) == 0);
	close(plain);
	close(closing);
	// A name of any characters, up to 249 bytes.
	char name[256];
	memset(name, 'n', sizeof(name));
	name[249] = 0;
	int longest = memfd_create(name, 0);
	CHECK(longest >= 0);
	close(longest);
	name[249] = 'n';
	name[250] = 0;
	CHECK_ERROR(memfd_create(name, 0), EINVAL);
	int slashes = memfd_create("a/b/../c", 0);
	CHECK(slashes >= 0);
	close(slashes);
	int empty = memfd_create("", 0);
	CHECK(empty >= 0);
	close(empty);
	// No name at all.
	const char *volatile none = NULL;
	CHECK_ERROR(memfd_create(none, 0), EFAULT);
	// Flags that are not Linux's, MFD_EXEC with MFD_NOEXEC_SEAL, and the
	// order of the checks: the flags first.
	CHECK_ERROR(memfd_create("a", 0x4000), EINVAL);
	CHECK_ERROR(memfd_create("a", 0x20), EINVAL);
	CHECK_ERROR(memfd_create("a", MFD_EXEC | MFD_NOEXEC_SEAL), EINVAL);
	CHECK_ERROR(memfd_create(none, 0x4000), EINVAL);
}

static void modes(void) {
	struct stat st;
	int runs = memfd_create("a", MFD_EXEC);
	CHECK(runs >= 0 && fstat(runs, &st) == 0);
	CHECK((st.st_mode & 0777) == 0777);
	close(runs);
	// No execute bits, and sealed so, which allows more seals.
	int never = memfd_create("a", MFD_NOEXEC_SEAL);
	CHECK(never >= 0 && fstat(never, &st) == 0);
	CHECK((st.st_mode & 0777) == 0666);
	CHECK(seals_of(never) == F_SEAL_EXEC);
	CHECK_ERROR(fchmod(never, 0777), EPERM);
	CHECK(fchmod(never, 0660) == 0);
	CHECK(fcntl(never, F_ADD_SEALS, F_SEAL_GROW) == 0);
	close(never);
}

static void shared_mapping(void) {
	int fd = sealable(2);
	char *map = map_shared(fd, 2, PROT_READ | PROT_WRITE);
	CHECK(map != MAP_FAILED);
	// What a write puts in the file is in the mapping, and the other way.
	CHECK(pwrite(fd, "from write", 10, 0) == 10);
	CHECK(!memcmp(map, "from write", 10));
	memcpy(map + PAGE, "from map", 8);
	char buffer[16];
	CHECK(pread(fd, buffer, 8, PAGE) == 8 && !memcmp(buffer, "from map", 8));
	// The mapping outlives the descriptor.
	close(fd);
	CHECK(!memcmp(map + PAGE, "from map", 8));
	// What a child writes through its copy of the mapping the parent sees,
	// and what the parent writes the child does.
	int up[2], down[2];
	CHECK(pipe(up) == 0 && pipe(down) == 0);
	pid_t child = check_fork();
	if(child == 0) {
		char byte;
		CHECK(read(down[0], &byte, 1) == 1);
		CHECK(!memcmp(map, "parent says", 11));
		memcpy(map + 100, "child says", 11);
		CHECK(write(up[1], "x", 1) == 1);
		check_exit_child();
	}
	memcpy(map, "parent says", 11);
	CHECK(write(down[1], "x", 1) == 1);
	char byte;
	CHECK(read(up[0], &byte, 1) == 1);
	CHECK(!memcmp(map + 100, "child says", 11));
	CHECK_CHILD(child);
	close(up[0]);
	close(up[1]);
	close(down[0]);
	close(down[1]);
	CHECK(munmap(map, 2 * PAGE) == 0);
}

static void shared_file_in_a_child(void) {
	int fd = sealable(1);
	pid_t child = check_fork();
	if(child == 0) {
		char *map = map_shared(fd, 1, PROT_READ | PROT_WRITE);
		CHECK(map != MAP_FAILED);
		memcpy(map, "child", 6);
		check_exit_child();
	}
	CHECK_CHILD(child);
	char buffer[8];
	CHECK(pread(fd, buffer, 6, 0) == 6 && !memcmp(buffer, "child", 6));
	close(fd);
}

static void without_sealing(void) {
	int fd = memfd_create("a", 0);
	CHECK(fd >= 0);
	CHECK(seals_of(fd) == F_SEAL_SEAL);
	CHECK_ERROR(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE), EPERM);
	CHECK_ERROR(fcntl(fd, F_ADD_SEALS, 0), EPERM);
	CHECK(seals_of(fd) == F_SEAL_SEAL);
	// It is written, and resized, as ever.
	CHECK(write(fd, "x", 1) == 1);
	CHECK(ftruncate(fd, 0) == 0);
	close(fd);
	fd = sealable(1);
	CHECK(seals_of(fd) == 0);
	CHECK(fcntl(fd, F_ADD_SEALS, 0) == 0);
	close(fd);
}

static void shrink_and_grow(void) {
	int fd = sealable(2);
	CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK) == 0);
	CHECK(seals_of(fd) == F_SEAL_SHRINK);
	CHECK_ERROR(ftruncate(fd, PAGE), EPERM);
	CHECK_ERROR(ftruncate(fd, 0), EPERM);
	// The same size, and a larger one, are not a shrinking.
	CHECK(ftruncate(fd, 2 * PAGE) == 0);
	CHECK(ftruncate(fd, 3 * PAGE) == 0);
	CHECK(write(fd, "x", 1) == 1);
	struct stat st;
	CHECK(fstat(fd, &st) == 0 && st.st_size == 3 * PAGE);
	close(fd);

	fd = sealable(2);
	CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_GROW) == 0);
	CHECK_ERROR(ftruncate(fd, 3 * PAGE), EPERM);
	CHECK(ftruncate(fd, 2 * PAGE) == 0);
	CHECK(ftruncate(fd, PAGE + 10) == 0);
	// A write inside the file is as ever. Linux checks each page it
	// writes: one that would grow the file is not done, so a write that
	// reaches past the page that holds the file's end is done as far as
	// the pages before it.
	CHECK(pwrite(fd, "in", 2, 0) == 2);
	CHECK(pwrite(fd, "inside", 6, PAGE + 4) == 6);
	CHECK_ERROR(pwrite(fd, "out", 3, PAGE + 9), EPERM);
	CHECK_ERROR(pwrite(fd, "out", 3, 2 * PAGE), EPERM);
	char twenty[20] = {0};
	CHECK(pwrite(fd, twenty, sizeof(twenty), PAGE - 5) == 5);
	CHECK(fstat(fd, &st) == 0 && st.st_size == PAGE + 10);
	close(fd);
}

static void write_seal(void) {
	int fd = sealable(1);
	CHECK(pwrite(fd, "before", 6, 0) == 6);
	CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == 0);
	CHECK(seals_of(fd) == F_SEAL_WRITE);
	CHECK_ERROR(write(fd, "x", 1), EPERM);
	CHECK_ERROR(pwrite(fd, "x", 1, 0), EPERM);
	// A write of no bytes is no write.
	CHECK(write(fd, "", 0) == 0);
	// Reading, and resizing, are as ever.
	char buffer[8];
	CHECK(pread(fd, buffer, 6, 0) == 6 && !memcmp(buffer, "before", 6));
	CHECK(ftruncate(fd, 2 * PAGE) == 0);
	// A shared mapping that may be written is refused, whatever it asks
	// for besides, and one that only reads is made, and stays so.
	CHECK_ERROR(mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0), EPERM);
	CHECK_ERROR(mmap(NULL, PAGE, PROT_WRITE, MAP_SHARED, fd, 0), EPERM);
	char *map = map_shared(fd, 1, PROT_READ);
	CHECK(map != MAP_FAILED);
	CHECK(!memcmp(map, "before", 6));
	CHECK_ERROR(mprotect(map, PAGE, PROT_READ | PROT_WRITE), EACCES);
	CHECK(munmap(map, PAGE) == 0);
	// A private one is a copy: it may be written, and the file is not.
	char *copy = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
	CHECK(copy != MAP_FAILED);
	memcpy(copy, "private", 8);
	CHECK(pread(fd, buffer, 6, 0) == 6 && !memcmp(buffer, "before", 6));
	CHECK(munmap(copy, PAGE) == 0);
	// The seal is added once, and seals added with it stay.
	CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE | F_SEAL_GROW) == 0);
	CHECK(seals_of(fd) == (F_SEAL_WRITE | F_SEAL_GROW));
	close(fd);
}

static void future_write_seal(void) {
	int fd = sealable(1);
	// A mapping made before the seal stays writable, and sees the file.
	char *map = map_shared(fd, 1, PROT_READ | PROT_WRITE);
	CHECK(map != MAP_FAILED);
	CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_FUTURE_WRITE) == 0);
	CHECK(seals_of(fd) == F_SEAL_FUTURE_WRITE);
	memcpy(map, "mapped", 7);
	char buffer[8];
	CHECK(pread(fd, buffer, 7, 0) == 7 && !memcmp(buffer, "mapped", 7));
	// What is made after is not written.
	CHECK_ERROR(write(fd, "x", 1), EPERM);
	CHECK_ERROR(mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0), EPERM);
	char *reading = map_shared(fd, 1, PROT_READ);
	CHECK(reading != MAP_FAILED);
	CHECK(!memcmp(reading, "mapped", 7));
	CHECK_ERROR(mprotect(reading, PAGE, PROT_READ | PROT_WRITE), EACCES);
	CHECK(munmap(reading, PAGE) == 0);
	CHECK(munmap(map, PAGE) == 0);
	// Write is then added with no mapping left.
	CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == 0);
	CHECK(seals_of(fd) == (F_SEAL_FUTURE_WRITE | F_SEAL_WRITE));
	close(fd);
}

static void write_seal_while_mapped(void) {
	int fd = sealable(1);
	char *map = map_shared(fd, 1, PROT_READ | PROT_WRITE);
	CHECK(map != MAP_FAILED);
	CHECK_ERROR(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE), EBUSY);
	// The refusal adds nothing, not even what came with the write seal.
	CHECK_ERROR(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE | F_SEAL_GROW), EBUSY);
	CHECK(seals_of(fd) == 0);
	// The file is still written and mapped.
	CHECK(write(fd, "x", 1) == 1);
	char *another = map_shared(fd, 1, PROT_READ | PROT_WRITE);
	CHECK(another != MAP_FAILED);
	CHECK(munmap(another, PAGE) == 0);
	CHECK(munmap(map, PAGE) == 0);
	CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == 0);
	close(fd);

	// A mapping of another process is as much an obstacle.
	fd = sealable(1);
	int up[2], down[2];
	CHECK(pipe(up) == 0 && pipe(down) == 0);
	pid_t child = check_fork();
	if(child == 0) {
		char *held = map_shared(fd, 1, PROT_READ | PROT_WRITE);
		CHECK(held != MAP_FAILED);
		CHECK(write(up[1], "x", 1) == 1);
		char byte;
		CHECK(read(down[0], &byte, 1) == 1);
		check_exit_child();
	}
	char byte;
	CHECK(read(up[0], &byte, 1) == 1);
	CHECK_ERROR(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE), EBUSY);
	CHECK(write(down[1], "x", 1) == 1);
	CHECK_CHILD(child);
	// The child has gone, and its mapping with it.
	CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == 0);
	close(fd);
	close(up[0]);
	close(up[1]);
	close(down[0]);
	close(down[1]);
}

static void exec_seal(void) {
	int fd = memfd_create("a", MFD_ALLOW_SEALING | MFD_EXEC);
	CHECK(fd >= 0);
	struct stat st;
	CHECK(fstat(fd, &st) == 0 && (st.st_mode & 0777) == 0777);
	CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_EXEC) == 0);
	// The mode may change as long as its execute bits do not.
	CHECK_ERROR(fchmod(fd, 0666), EPERM);
	CHECK_ERROR(fchmod(fd, 0770), EPERM);
	CHECK(fchmod(fd, 0777) == 0);
	CHECK(fchmod(fd, 0755) == 0);
	CHECK(fstat(fd, &st) == 0 && (st.st_mode & 0777) == 0755);
	close(fd);
}

static void seal_seal(void) {
	int fd = sealable(1);
	CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK) == 0);
	CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_SEAL) == 0);
	CHECK(seals_of(fd) == (F_SEAL_SHRINK | F_SEAL_SEAL));
	// Nothing is added once the file is sealed with seal, not even a seal
	// it has, and nothing is added by none.
	CHECK_ERROR(fcntl(fd, F_ADD_SEALS, F_SEAL_GROW), EPERM);
	CHECK_ERROR(fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK), EPERM);
	CHECK_ERROR(fcntl(fd, F_ADD_SEALS, 0), EPERM);
	CHECK(seals_of(fd) == (F_SEAL_SHRINK | F_SEAL_SEAL));
	// A copy of the descriptor is the same file with the same seals.
	int copy = dup(fd);
	CHECK(copy >= 0 && seals_of(copy) == (F_SEAL_SHRINK | F_SEAL_SEAL));
	close(copy);
	close(fd);
}

static void seal_arguments(void) {
	int fd = sealable(1);
	// A bit that is no seal is no seal, whatever else is asked.
	CHECK_ERROR(fcntl(fd, F_ADD_SEALS, 0x40), EINVAL);
	CHECK_ERROR(fcntl(fd, F_ADD_SEALS, F_SEAL_GROW | 0x40), EINVAL);
	CHECK(seals_of(fd) == 0);
	// All of them together.
	CHECK(fcntl(fd, F_ADD_SEALS,
			F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE | F_SEAL_FUTURE_WRITE
					| F_SEAL_EXEC) == 0);
	CHECK(seals_of(fd) == 0x3f);
	close(fd);

	// Only memory files have seals.
	int plain = open("/tmp/memfd-plain", O_RDWR | O_CREAT | O_TRUNC, 0600);
	CHECK(plain >= 0);
	CHECK_ERROR(fcntl(plain, F_GET_SEALS), EINVAL);
	CHECK_ERROR(fcntl(plain, F_ADD_SEALS, F_SEAL_GROW), EINVAL);
	close(plain);
	unlink("/tmp/memfd-plain");
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK_ERROR(fcntl(fds[0], F_GET_SEALS), EINVAL);
	CHECK_ERROR(fcntl(fds[1], F_ADD_SEALS, F_SEAL_GROW), EINVAL);
	close(fds[0]);
	close(fds[1]);
	CHECK_ERROR(fcntl(-1, F_GET_SEALS), EBADF);
}

int main(void) {
	check_start("memfd");
	RUN(reads_and_writes);
	RUN(flags_and_names);
	RUN(modes);
	RUN(shared_mapping);
	RUN(shared_file_in_a_child);
	RUN(without_sealing);
	RUN(shrink_and_grow);
	RUN(write_seal);
	RUN(future_write_seal);
	RUN(write_seal_while_mapped);
	RUN(exec_seal);
	RUN(seal_seal);
	RUN(seal_arguments);
	return check_end();
}
