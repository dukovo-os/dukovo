// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Memory that is writable and executable at once, as a JIT compiler makes
// it (memory.jit, ADR 0624): a private anonymous page mapped with
// PROT_READ, PROT_WRITE and PROT_EXEC runs the code written into it, and
// runs new code written over it; a page written and then turned
// executable with mprotect runs, and so does one turned writable and
// executable; the pages stay the process's own across fork, each side
// writing its own copy; and a program started with execve keeps what its
// process was given.
//
// It is a Linux program, never one of Dukovo's: the linux-programs boot
// test builds it with the build machine's C library and with musl, and
// runs it three ways on Dukovo. Run by its name, its process has no
// memory.jit, and every writable and executable mapping is EACCES. Run
// with `--given` through `linux run --allow-jit`, the private anonymous
// ones work, and a file's pages and shared memory are EACCES still. Run
// with `--refused` through plain `linux run`, it asks once, as a JIT that
// gives up does, and ends with status 3, after which `linux run` says
// that --allow-jit would have let it. On a Linux machine, where nothing
// refuses such memory, every mapping works, which is how the checks of
// the code that runs are known to be right.

#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <fcntl.h>
#include <stdint.h>
#include <sys/mman.h>
#include <sys/utsname.h>

#include "check.h"

#define PAGE 4096

// What the process may have: Linux gives every process such memory, and
// Dukovo only one given memory.jit.
static int allowed;
// Whether it runs on Dukovo, as uname's version says.
static int on_dukovo;
// The program's own path, to run itself with execve.
static const char *self;

// `mov eax, value; ret`: the code written into the pages.
static void write_code(unsigned char *at, uint32_t value) {
	at[0] = 0xb8;
	memcpy(at + 1, &value, sizeof(value));
	at[5] = 0xc3;
}

// Runs the code at `at`.
static int run_code(const unsigned char *at) {
	int (*code)(void);
	memcpy(&code, &at, sizeof(code));
	return code();
}

// mmap with PROT_READ, PROT_WRITE and PROT_EXEC, checked to work where
// `works` and to be EACCES elsewhere; the page, or NULL.
static unsigned char *map_rwx(int flags, int fd, int works) {
	void *page = mmap(NULL, PAGE, PROT_READ | PROT_WRITE | PROT_EXEC, flags, fd, 0);
	if(works) {
		CHECK(page != MAP_FAILED);
	} else {
		CHECK(page == MAP_FAILED && errno == EACCES);
	}
	return page == MAP_FAILED ? NULL : page;
}

// --- Private anonymous memory ---

// A page mapped writable and executable runs the code written into it,
// and the code written over it after.
static void anonymous(void) {
	unsigned char *page = map_rwx(MAP_PRIVATE | MAP_ANONYMOUS, -1, allowed);
	if(!page)
		return;
	write_code(page, 42);
	CHECK(run_code(page) == 42);
	write_code(page, 7);
	CHECK(run_code(page) == 7);
	// Code further into the page, as a JIT adds functions.
	write_code(page + 100, 9);
	CHECK(run_code(page + 100) == 9 && run_code(page) == 7);
	CHECK(munmap(page, PAGE) == 0);
}

// A page written, then turned executable, runs whatever the process has;
// turned writable and executable, only with memory.jit.
static void turned(void) {
	unsigned char *page =
			mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(page != MAP_FAILED);
	if(page == MAP_FAILED)
		return;
	write_code(page, 42);
	CHECK(mprotect(page, PAGE, PROT_READ | PROT_EXEC) == 0);
	CHECK(run_code(page) == 42);
	if(allowed) {
		CHECK(mprotect(page, PAGE, PROT_READ | PROT_WRITE | PROT_EXEC) == 0);
		write_code(page, 43);
		CHECK(run_code(page) == 43);
	} else {
		CHECK_ERROR(mprotect(page, PAGE, PROT_READ | PROT_WRITE | PROT_EXEC), EACCES);
		// Still executable, and not writable.
		CHECK(run_code(page) == 42);
	}
	CHECK(mprotect(page, PAGE, PROT_READ | PROT_WRITE) == 0);
	write_code(page, 44);
	CHECK(mprotect(page, PAGE, PROT_READ | PROT_EXEC) == 0);
	CHECK(run_code(page) == 44);
	CHECK(munmap(page, PAGE) == 0);
}

// --- What is never writable and executable on Dukovo ---

// A file's pages, private or shared.
static void file_pages(void) {
	char path[] = "/tmp/jit-XXXXXX";
	int fd = mkstemp(path);
	CHECK(fd >= 0);
	if(fd < 0)
		return;
	unlink(path);
	unsigned char code[PAGE] = {0};
	write_code(code, 42);
	CHECK(write(fd, code, sizeof(code)) == (ssize_t)sizeof(code));
	for(int i = 0; i < 2; i++) {
		int flags = i == 0 ? MAP_PRIVATE : MAP_SHARED;
		unsigned char *page = map_rwx(flags, fd, !on_dukovo);
		if(page) {
			CHECK(run_code(page) == 42);
			CHECK(munmap(page, PAGE) == 0);
		}
	}
	// Mapped readable and executable, it runs, and mprotect cannot make
	// it writable too.
	unsigned char *page = mmap(NULL, PAGE, PROT_READ | PROT_EXEC, MAP_PRIVATE, fd, 0);
	CHECK(page != MAP_FAILED);
	if(page != MAP_FAILED) {
		CHECK(run_code(page) == 42);
		int writable = mprotect(page, PAGE, PROT_READ | PROT_WRITE | PROT_EXEC);
		if(on_dukovo)
			CHECK(writable == -1 && errno == EACCES);
		else
			CHECK(writable == 0);
		CHECK(munmap(page, PAGE) == 0);
	}
	close(fd);
}

// Shared anonymous memory, which another process may write.
static void shared_anonymous(void) {
	unsigned char *page = map_rwx(MAP_SHARED | MAP_ANONYMOUS, -1, !on_dukovo);
	if(page) {
		write_code(page, 42);
		CHECK(run_code(page) == 42);
		CHECK(munmap(page, PAGE) == 0);
	}
}

// --- Across fork and execve ---

// The child of a fork may have the same memory, and the page it had
// before is copied when either side writes it: each runs its own code.
static void forked(void) {
	unsigned char *page = map_rwx(MAP_PRIVATE | MAP_ANONYMOUS, -1, allowed);
	if(page)
		write_code(page, 42);
	pid_t pid = check_fork();
	if(pid == 0) {
		if(page) {
			CHECK(run_code(page) == 42);
			write_code(page, 7);
			CHECK(run_code(page) == 7);
		}
		unsigned char *own = map_rwx(MAP_PRIVATE | MAP_ANONYMOUS, -1, allowed);
		if(own) {
			write_code(own, 8);
			CHECK(run_code(own) == 8);
		}
		check_exit_child();
	}
	int status = 0;
	CHECK(waitpid(pid, &status, 0) == pid);
	CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
	if(page) {
		// The child's write was its own.
		CHECK(run_code(page) == 42);
		write_code(page, 5);
		CHECK(run_code(page) == 5);
		CHECK(munmap(page, PAGE) == 0);
	}
}

// A program the process starts keeps what the process was given.
static void executed(void) {
	pid_t pid = check_fork();
	if(pid == 0) {
		char *args[] = {(char *)self, allowed ? "--executed-given" : "--executed", NULL};
		execv(self, args);
		printf("%s: execv: %s\n", check_name, strerror(errno));
		_exit(100);
	}
	int status = 0;
	CHECK(waitpid(pid, &status, 0) == pid);
	CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

// The program run again by execve: its page works where its process may.
static int executed_again(void) {
	check_start("jit (executed)");
	RUN(anonymous);
	return check_end();
}

// Run through plain `linux run`: asks once, as a JIT does, and gives up.
static int refused(void) {
	void *page = mmap(NULL, PAGE, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS,
			-1, 0);
	if(page != MAP_FAILED) {
		printf("jit: the page was given\n");
		return on_dukovo ? 1 : 0;
	}
	printf("jit: no writable and executable memory: %s\n", strerror(errno));
	return errno == EACCES ? 3 : 1;
}

int main(int argc, char **argv) {
	self = argv[0];
	struct utsname names;
	on_dukovo = uname(&names) == 0 && strstr(names.version, "Dukovo") != NULL;
	const char *mode = argc > 1 ? argv[1] : "";
	int given = strcmp(mode, "--given") == 0 || strcmp(mode, "--executed-given") == 0;
	allowed = !on_dukovo || given;
	if(strcmp(mode, "--refused") == 0)
		return refused();
	if(strncmp(mode, "--executed", 10) == 0)
		return executed_again();
	check_start(given ? "jit (given)" : "jit");
	printf("%s: running on %s\n", check_name, on_dukovo ? "Dukovo" : "Linux");
	RUN(anonymous);
	RUN(turned);
	RUN(file_pages);
	RUN(shared_anonymous);
	RUN(forked);
	RUN(executed);
	return check_end();
}
