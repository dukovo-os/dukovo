// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The C library's corners that the post-Phase-9 audit fixed, each as
// glibc 2.39 has it on Linux: allocations near the top of the address
// space and on large alignments, a null environ, popen, message
// catalogs, the functions that were "Not implemented", and threads'
// stacks and signal masks; and those CPython's test suite found (ADR
// 0495): sysconf's answers, writev and readv of no buffers, strsignal's
// words, pathconf's errors, the syslog mask, fma's sign of zero, dladdr
// in a static program, and getaddrinfo's service names and IPv6 scopes;
// and one that PostgreSQL's setup found (ADR 0521): fflush(NULL) keeps
// what a stream reading a pipe has read ahead; and what PHP's suite found:
// a stream whose command ended unread closes without a word; and a block
// that realloc shrinks a long way gives back what it no longer needs, as
// CPython's pipe reads want; and each thread's cache of freed blocks
// (ADR 0657); and the large blocks kept after free (ADR 0810).

#include <dlfcn.h>
#include <fcntl.h>
#include <limits.h>
#include <malloc.h>
#include <math.h>
#include <net/if.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/ether.h>
#include <nl_types.h>
#include <pthread.h>
#include <stdint.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <sys/utsname.h>
#include <syslog.h>

#include "check.h"

extern char **environ;

// `size`, which the compiler cannot see through, so that it neither
// warns of nor folds an allocation it knows must fail.
static size_t unknown(size_t size) {
	volatile size_t hidden = size;
	return hidden;
}

// The allocation functions, called through pointers the compiler cannot
// see through either: it takes an allocation nobody reads as one it may
// leave out, and that it then gave a block (clang -O2).
static void *(*volatile malloc_)(size_t) = malloc;
static void *(*volatile calloc_)(size_t, size_t) = calloc;
static void *(*volatile realloc_)(void *, size_t) = realloc;
static void *(*volatile aligned_alloc_)(size_t, size_t) = aligned_alloc;
static void *(*volatile memalign_)(size_t, size_t) = memalign;

// memset likewise, where what it fills is freed with nobody having read
// it: the compiler leaves such a fill out, and the block's pages are
// then never touched.
static void *(*volatile memset_)(void *, int, size_t) = memset;

static int aligned(const void *p, size_t alignment) {
	return ((uintptr_t)p & (alignment - 1)) == 0;
}

static void allocations(void) {
	// What cannot be had fails with ENOMEM, and never wraps to a small
	// block handed out as a large one.
	static const size_t huge[] = {SIZE_MAX, SIZE_MAX - 4096, SIZE_MAX - 300000,
			(size_t)PTRDIFF_MAX + 1};
	for(size_t i = 0; i < sizeof(huge) / sizeof(huge[0]); i++) {
		size_t size = unknown(huge[i]);
		errno = 0;
		CHECK(malloc_(size) == NULL && errno == ENOMEM);
		// A failed realloc leaves the block as it was.
		char *p = malloc_(16);
		CHECK(p != NULL);
		strcpy(p, "kept");
		errno = 0;
		char *bigger = realloc_(p, size);
		CHECK(bigger == NULL && errno == ENOMEM);
		if(bigger)
			p = bigger;
		CHECK(strcmp(p, "kept") == 0);
		free(p);
		void *q = &q;
		CHECK(posix_memalign(&q, 4096, size) == ENOMEM);
	}
	errno = 0;
	CHECK(calloc_(unknown(SIZE_MAX / 2), 4) == NULL && errno == ENOMEM);
	// Every power-of-two alignment, small blocks and large.
	static const size_t sizes[] = {1, 100, 4096, 65536, 300000};
	for(size_t alignment = sizeof(void *); alignment <= (1 << 20); alignment <<= 1) {
		for(size_t i = 0; i < sizeof(sizes) / sizeof(sizes[0]); i++) {
			void *p = NULL;
			CHECK(posix_memalign(&p, alignment, sizes[i]) == 0 && p && aligned(p, alignment));
			if(p) {
				memset(p, 0x5a, sizes[i]);
				CHECK(malloc_usable_size(p) >= sizes[i]);
				// realloc keeps what was there.
				unsigned char *bigger = realloc_(p, sizes[i] * 2);
				CHECK(bigger && bigger[sizes[i] - 1] == 0x5a);
				free(bigger);
			}
			void *a = aligned_alloc_(alignment, sizes[i]);
			CHECK(a && aligned(a, alignment));
			free(a);
			void *m = memalign_(alignment, sizes[i]);
			CHECK(m && aligned(m, alignment));
			free(m);
		}
	}
	void *p = &p;
	CHECK(posix_memalign(&p, 3, 8) == EINVAL && p == &p);
	CHECK(posix_memalign(&p, sizeof(void *) / 2, 8) == EINVAL);
	errno = 0;
	CHECK(aligned_alloc_(unknown(3), 8) == NULL && errno == EINVAL);
	// memalign rounds an alignment up to a power of two.
	void *m = memalign_(48, 100);
	CHECK(m && aligned(m, 64));
	free(m);
	// C17: any size, not only a multiple of the alignment.
	void *odd = aligned_alloc_(64, 100);
	CHECK(odd && aligned(odd, 64));
	free(odd);
}

// realloc moves a block that shrinks a long way into one of the size it
// now needs, and keeps one that shrinks a little where it is. CPython
// reads a pipe into 32 KiB and shrinks the block to what came: kept
// whole, each read held the block's pages and the page of its header, so
// a subprocess whose output came a hundred bytes at a time ran the C
// library out of memory (Python's test_threading, gh-102056).
static void shrinks(void) {
	enum { BLOCKS = 2000, READ = 32 * 1024 + 33, CAME = 105 };
	static char *blocks[BLOCKS];
	long before = check_anonymous_pages();
	CHECK(before > 0);
	int lost = 0;
	for(int i = 0; i < BLOCKS; i++) {
		char *buffer = malloc_(READ);
		CHECK(buffer != NULL);
		if(!buffer)
			break;
		memset(buffer, 'a' + i % 26, CAME);
		blocks[i] = realloc_(buffer, CAME);
		if(!blocks[i] || blocks[i][0] != 'a' + i % 26 || blocks[i][CAME - 1] != 'a' + i % 26
				|| malloc_usable_size(blocks[i]) != 128)
			lost++;
	}
	CHECK(lost == 0);
	long after = check_anonymous_pages();
	// 2000 blocks of 128 bytes take 63 pages; kept whole, over 4000.
	if(after - before >= 200)
		fprintf(stderr, "library: %ld pages before %d shrunk reads, %ld after\n", before,
				BLOCKS, after);
	CHECK(after - before < 200);
	for(int i = 0; i < BLOCKS; i++)
		free(blocks[i]);

	// A slab object that a smaller bucket holds moves there, with what it
	// held; one that still needs its bucket stays.
	char *object = malloc_(4096);
	CHECK(object != NULL);
	if(object) {
		memset(object, 'o', 4096);
		char *half = realloc_(object, 2048);
		CHECK(half && half != object && malloc_usable_size(half) == 2048 && half[0] == 'o'
				&& half[2047] == 'o');
		char *same = realloc_(half, 1500);
		CHECK(same == half && malloc_usable_size(same) == 2048);
		free(same);
	}
	// A large block that needs half its pages or fewer moves, rounded to
	// pages; one that needs more stays.
	char *large = malloc_(1 << 20);
	CHECK(large != NULL);
	if(large) {
		memset(large, 'l', 1 << 20);
		char *most = realloc_(large, 600 * 1024);
		CHECK(most == large && malloc_usable_size(most) == 1 << 20);
		char *less = realloc_(most, 300 * 1024 + 1);
		CHECK(less && less != most && malloc_usable_size(less) == 304 * 1024
				&& less[0] == 'l' && less[300 * 1024] == 'l');
		free(less);
	}
}

// Frees four blocks of the smallest size the pool maps by itself, which
// push out whatever large blocks were kept, and returns the pages of
// anonymous memory the process then holds.
static long flushed_kept_blocks(void) {
	unsigned char *small[4];
	for(int i = 0; i < 4; i++) {
		small[i] = malloc_(40000);
		if(small[i])
			memset_(small[i], 0x66, 40000);
	}
	for(int i = 0; i < 4; i++)
		free(small[i]);
	return check_anonymous_pages();
}

// The last large blocks that were freed are kept, with their pages, and
// handed out again to a request they fit (ADR 0810): at most four of
// them and 16 MiB in all, and a kept block holds a request with at most
// a quarter as much again left over.
static void kept_blocks(void) {
	enum { FRAME = 1228800, MIB = 1 << 20 };
	// One of the size again is the block that was freed, and filling it
	// takes no page fault: its pages stayed.
	unsigned char *frame = malloc_(FRAME);
	CHECK(frame != NULL);
	if(!frame)
		return;
	memset_(frame, 0x11, FRAME);
	free(frame);
	unsigned char *again = malloc_(FRAME);
	CHECK(again == frame);
	if(again == frame) {
		long before = check_page_faults();
		memset_(again, 0x22, FRAME);
		long faults = check_page_faults() - before;
		if(faults != 0)
			fprintf(stderr, "library: %ld page faults filling a kept block\n", faults);
		CHECK(before >= 0 && faults == 0);
	}
	free(again);
	// A little smaller fits it, and says how much it has; much smaller
	// or any larger is a block of its own.
	unsigned char *smaller = malloc_(FRAME - FRAME / 8);
	CHECK(smaller == frame && malloc_usable_size(smaller) == FRAME);
	free(smaller);
	unsigned char *half = malloc_(FRAME / 2);
	CHECK(half && half != frame && malloc_usable_size(half) == FRAME / 2);
	unsigned char *larger = malloc_(FRAME + 4096);
	CHECK(larger && larger != frame && malloc_usable_size(larger) == FRAME + 4096);
	// calloc of a kept block is zeroes.
	free(half);
	free(larger);
	unsigned char *zeroes = calloc_(1, FRAME);
	CHECK(zeroes == frame);
	int dirty = 0;
	for(size_t i = 0; zeroes && i < FRAME; i++)
		dirty |= zeroes[i];
	CHECK(dirty == 0);
	free(zeroes);
	// A block that grows by realloc leaves the one it was in to be had
	// again.
	unsigned char *grown = malloc_(FRAME);
	CHECK(grown == frame);
	if(grown)
		memset_(grown, 0x33, FRAME);
	unsigned char *twice = realloc_(grown, 2 * FRAME);
	CHECK(twice && twice != frame && twice[0] == 0x33 && twice[FRAME - 1] == 0x33);
	free(twice);
	unsigned char *back = malloc_(FRAME);
	CHECK(back == frame);
	free(back);

	// Six blocks of 3 MiB freed leave four of them, with a page of the
	// pool's before each; the two freed first went back to the system.
	enum { EACH = 3 * MIB / 4096 + 1 };
	long before = flushed_kept_blocks();
	unsigned char *blocks[6];
	for(int i = 0; i < 6; i++) {
		blocks[i] = malloc_(3 * MIB);
		CHECK(blocks[i] != NULL);
		if(blocks[i])
			memset_(blocks[i], 0x44, 3 * MIB);
	}
	for(int i = 0; i < 6; i++)
		free(blocks[i]);
	long kept = check_anonymous_pages() - before;
	if(kept > 4 * EACH || kept < 4 * EACH - 64)
		fprintf(stderr, "library: %ld pages kept of six blocks of 3 MiB\n", kept);
	CHECK(before > 0 && kept <= 4 * EACH && kept >= 4 * EACH - 64);

	// A block of more than 16 MiB is never kept.
	before = flushed_kept_blocks();
	unsigned char *huge = malloc_(17 * MIB);
	CHECK(huge != NULL);
	if(huge)
		memset_(huge, 0x55, MIB);
	free(huge);
	kept = check_anonymous_pages() - before;
	if(kept > 8)
		fprintf(stderr, "library: %ld pages kept of a block of 17 MiB\n", kept);
	CHECK(before > 0 && kept <= 8);
}

// Each thread's cache of freed blocks (ADR 0657). Threads allocate and
// free blocks of every size the caches keep and some they do not, hand
// blocks to each other to free, and check that no block changed while
// it was theirs.
enum { CACHE_THREADS = 4, CACHE_ROUNDS = 200000, CACHE_LIVE = 256, CACHE_SHARED = 64 };

struct cached_block {
	unsigned char *p;
	size_t size;
	unsigned char fill;
};

static pthread_mutex_t shared_lock = PTHREAD_MUTEX_INITIALIZER;
static struct cached_block shared_blocks[CACHE_SHARED];
static int cache_damage;

static void check_and_free(struct cached_block *block) {
	for(size_t i = 0; i < block->size; i++) {
		if(block->p[i] != block->fill) {
			__atomic_fetch_add(&cache_damage, 1, __ATOMIC_RELAXED);
			break;
		}
	}
	free(block->p);
	block->p = NULL;
}

static void *cache_user(void *argument) {
	uint64_t seed = (uintptr_t)argument * 2654435761u + 1;
	struct cached_block live[CACHE_LIVE] = {0};
	for(int i = 0; i < CACHE_ROUNDS; i++) {
		seed = seed * 6364136223846793005ull + 1442695040888963407ull;
		unsigned r = (unsigned)(seed >> 32);
		struct cached_block *block = &live[r % CACHE_LIVE];
		if(block->p && (r & (1u << 20))) {
			// Swapped with one another thread may have left.
			pthread_mutex_lock(&shared_lock);
			struct cached_block *other = &shared_blocks[(r >> 8) % CACHE_SHARED];
			struct cached_block mine = *block;
			*block = *other;
			*other = mine;
			pthread_mutex_unlock(&shared_lock);
		}else if(block->p) {
			check_and_free(block);
		}else{
			size_t size = (r >> 4) % (r & 1 ? 600 : 5000);
			if(r & (1u << 21)) {
				size_t alignment = (size_t)8 << ((r >> 12) % 10);
				void *q = NULL;
				if(posix_memalign(&q, alignment, size) || !aligned(q, alignment))
					__atomic_fetch_add(&cache_damage, 1, __ATOMIC_RELAXED);
				block->p = q;
			}else{
				block->p = malloc_(size);
			}
			if(!block->p) {
				__atomic_fetch_add(&cache_damage, 1, __ATOMIC_RELAXED);
				continue;
			}
			block->size = size;
			block->fill = (unsigned char)(r >> 24) | 1;
			memset(block->p, block->fill, size);
		}
	}
	for(int i = 0; i < CACHE_LIVE; i++)
		if(live[i].p)
			check_and_free(&live[i]);
	return NULL;
}

// Frees 16 blocks of each size a cache keeps, as many as it holds, and
// exits; or, with an argument, exits at once.
static void *cache_filler(void *argument) {
	if(argument)
		return NULL;
	void *blocks[16];
	for(size_t size = 16; size <= 4096; size *= 2) {
		for(int i = 0; i < 16; i++)
			blocks[i] = malloc_(size);
		for(int i = 0; i < 16; i++)
			free(blocks[i]);
	}
	return NULL;
}

static void double_free(void) {
	char *p = malloc_(40);
	free(p);
	free(p);
}

static void dangling_write(void) {
	char *p = malloc_(72);
	char *q = malloc_(72);
	free(p);
	free(q);
	// One bit of the link the cache keeps in a freed block's first word,
	// which then points at no block: malloc finds it as it hands q out.
	*(volatile char *)q ^= 1;
	malloc_(72);
	malloc_(72);
}

// Runs `part` in a child, which must end on SIGABRT.
static void check_aborts(void (*part)(void), const char *what) {
	pid_t pid = check_fork();
	if(pid == 0) {
		// The C library's message, which would go to the console.
		int null = open("/dev/null", O_WRONLY);
		if(null >= 0)
			dup2(null, 2);
		part();
		_exit(0);
	}
	if(pid < 0)
		return;
	int status = check_wait(pid);
	if(!WIFSIGNALED(status) || WTERMSIG(status) != SIGABRT)
		printf("%s: %s ended with status %#x\n", check_name, what, status);
	CHECK(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT);
}

static void thread_caches(void) {
	pthread_t threads[CACHE_THREADS];
	for(uintptr_t i = 0; i < CACHE_THREADS; i++)
		CHECK(pthread_create(&threads[i], NULL, cache_user, (void *)(i + 1)) == 0);
	for(int i = 0; i < CACHE_THREADS; i++)
		CHECK(pthread_join(threads[i], NULL) == 0);
	for(int i = 0; i < CACHE_SHARED; i++)
		if(shared_blocks[i].p)
			check_and_free(&shared_blocks[i]);
	CHECK(cache_damage == 0);

	// A thread's cache goes back to the pool as the thread exits: kept by
	// 300 threads that are gone, the blocks would be over 4,000 pages more
	// than 300 threads that allocate nothing leave.
	long grew[2];
	for(int filled = 0; filled < 2; filled++) {
		long before = check_anonymous_pages();
		CHECK(before > 0);
		for(int i = 0; i < 300; i++) {
			pthread_t thread;
			CHECK(pthread_create(&thread, NULL, cache_filler, filled ? NULL : &grew) == 0);
			CHECK(pthread_join(thread, NULL) == 0);
		}
		grew[filled] = check_anonymous_pages() - before;
	}
	if(grew[1] - grew[0] >= 400)
		printf("%s: 300 threads grew the process by %ld pages, and by %ld when each filled "
				"its cache\n", check_name, grew[0], grew[1]);
	CHECK(grew[1] - grew[0] < 400);

	check_aborts(double_free, "a double free");
	check_aborts(dangling_write, "a write to a freed block's link");
}

static void environment(void) {
	CHECK(setenv("LIBRARY_TEST", "1", 1) == 0);
	// A copy of the environment, to put back.
	size_t count = 0;
	while(environ[count])
		count++;
	char **saved = calloc(count + 1, sizeof(char *));
	for(size_t i = 0; saved && i < count; i++)
		saved[i] = strdup(environ[i]);
	// clearenv(3) names this as the other way to clear the environment.
	environ = NULL;
	CHECK(getenv("LIBRARY_TEST") == NULL);
	CHECK(getenv("PATH") == NULL);
	CHECK(unsetenv("LIBRARY_TEST") == 0);
	CHECK(setenv("LIBRARY_ALONE", "2", 1) == 0);
	CHECK(getenv("LIBRARY_ALONE") && strcmp(getenv("LIBRARY_ALONE"), "2") == 0);
	CHECK(environ && environ[0] && strcmp(environ[0], "LIBRARY_ALONE=2") == 0 && !environ[1]);
	CHECK(clearenv() == 0);
	for(size_t i = 0; saved && i < count; i++)
		putenv(saved[i]);
	free(saved);
	CHECK(unsetenv("LIBRARY_TEST") == 0);
	size_t now = 0;
	while(environ[now])
		now++;
	CHECK(now == count - 1);
	// realpath with no buffer gives one to free; without a path it is
	// EINVAL.
	char *real = realpath("/tmp/../tmp/.", NULL);
	CHECK(real && strcmp(real, "/tmp") == 0);
	free(real);
	char buffer[PATH_MAX];
	CHECK_ERROR(realpath(NULL, buffer) ? 0 : -1, EINVAL);
}

// Runs `command` in sh, as popen does; says what it printed, up to
// `room` bytes.
static int output_of(const char *command, char *text, size_t room) {
	FILE *f = popen(command, "r");
	if(!f)
		return -1;
	size_t n = fread(text, 1, room - 1, f);
	text[n] = '\0';
	return pclose(f);
}

static void popens(void) {
	// glibc's modes: 'r' or 'w', and 'e' for close-on-exec, in any order.
	static const char *const good[] = {"r", "w", "re", "we", "er", "rr", "ree"};
	static const int cloexec[] = {0, 0, 1, 1, 1, 0, 1};
	for(size_t i = 0; i < sizeof(good) / sizeof(good[0]); i++) {
		FILE *f = popen("true", good[i]);
		CHECK(f != NULL);
		if(!f)
			continue;
		int flags = fcntl(fileno(f), F_GETFD);
		CHECK(flags >= 0 && !!(flags & FD_CLOEXEC) == cloexec[i]);
		CHECK(pclose(f) == 0);
	}
	static const char *const bad[] = {"rw", "wr", "rx", "", "r+", "w+", "rb", "e", "W"};
	for(size_t i = 0; i < sizeof(bad) / sizeof(bad[0]); i++)
		CHECK_ERROR(popen("true", bad[i]) ? 0 : -1, EINVAL);

	char text[256];
	CHECK(output_of("echo hello; exit 3", text, sizeof(text)) == 3 << 8);
	CHECK(strcmp(text, "hello\n") == 0);

	// POSIX: a later popen's child has none of the streams earlier ones
	// opened, so a command reading from one sees its end when it is
	// closed, whatever else runs.
	FILE *writer = popen("cat > /dev/null", "w");
	CHECK(writer != NULL);
	if(writer) {
		char command[128];
		snprintf(command, sizeof(command),
				"if { true >&%d; } 2>/dev/null; then echo open; else echo closed; fi",
				fileno(writer));
		CHECK(output_of(command, text, sizeof(text)) == 0);
		CHECK(strcmp(text, "closed\n") == 0);
		CHECK(pclose(writer) == 0);
	}

	// With its standard input closed, the pipe's end the command reads
	// from is born as descriptor 0, where it stays.
	pid_t child = check_fork();
	if(child == 0) {
		close(STDIN_FILENO);
		FILE *in = popen("cat > /tmp/library-popen", "w");
		CHECK(in != NULL);
		if(in) {
			fputs("through stdin\n", in);
			CHECK(pclose(in) == 0);
		}
		FILE *back = fopen("/tmp/library-popen", "r");
		CHECK(back && fgets(text, sizeof(text), back) && strcmp(text, "through stdin\n") == 0);
		if(back)
			fclose(back);
		unlink("/tmp/library-popen");
		check_exit_child();
	}
	CHECK_CHILD(child);

	// A command that ends without reading what was written to it: pclose
	// drops the bytes it cannot write (EPIPE), as glibc does, says nothing
	// on standard error, and gives the command's status. The command holds
	// the write end of `exited` until it ends, so the read that finds that
	// pipe closed shows it has, before anything is written.
	int exited[2];
	int made = pipe(exited) == 0;
	CHECK(made);
	if(made) {
		struct sigaction ignore = {.sa_handler = SIG_IGN}, old;
		CHECK(sigaction(SIGPIPE, &ignore, &old) == 0);
		FILE *gone = popen("exit 1", "w");
		close(exited[1]);
		CHECK(gone != NULL);
		if(gone) {
			char byte;
			CHECK(read(exited[0], &byte, 1) == 0);
			CHECK(fputs("nobody reads this\n", gone) >= 0);
			int quiet = open("/tmp/library-pclose", O_RDWR | O_CREAT | O_TRUNC, 0600);
			int stderr_saved = dup(STDERR_FILENO);
			if(quiet >= 0 && stderr_saved >= 0) {
				dup2(quiet, STDERR_FILENO);
				int status = pclose(gone);
				dup2(stderr_saved, STDERR_FILENO);
				CHECK(status == 1 << 8);
				CHECK(lseek(quiet, 0, SEEK_END) == 0);
			} else {
				CHECK(quiet >= 0 && stderr_saved >= 0);
				pclose(gone);
			}
			if(quiet >= 0)
				close(quiet);
			if(stderr_saved >= 0)
				close(stderr_saved);
			unlink("/tmp/library-pclose");
		}
		sigaction(SIGPIPE, &old, NULL);
		close(exited[0]);
	}
}

// fflush(NULL), which a program calls before it forks or runs popen,
// writes the streams with pending writes alone, as glibc and musl do: a
// stream reading a pipe keeps what it has read ahead, which the pipe
// cannot give again. initdb feeds PostgreSQL's single-user server its
// setup through a pipe, and the server runs popen as it reads it.
static void flush_all(void) {
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(write(fds[1], "first\nsecond\nthird\n", 19) == 19);
	close(fds[1]);
	FILE *in = fdopen(fds[0], "r");
	CHECK(in != NULL);
	if(!in)
		return;
	// The first line's read takes the whole of the pipe into the buffer.
	char line[32];
	CHECK(fgets(line, sizeof(line), in) && strcmp(line, "first\n") == 0);
	CHECK(fflush(NULL) == 0);
	CHECK(fgets(line, sizeof(line), in) && strcmp(line, "second\n") == 0);
	CHECK(fflush_unlocked(NULL) == 0);
	CHECK(fgets(line, sizeof(line), in) && strcmp(line, "third\n") == 0);
	CHECK(fgetc(in) == EOF && feof(in));
	fclose(in);

	// A stream with pending writes is written by either.
	CHECK(pipe(fds) == 0);
	FILE *out = fdopen(fds[1], "w");
	CHECK(out != NULL);
	if(!out)
		return;
	char text[16] = {0};
	CHECK(fputs("pending", out) >= 0);
	CHECK(fflush(NULL) == 0);
	CHECK(read(fds[0], text, sizeof(text)) == 7 && memcmp(text, "pending", 7) == 0);
	CHECK(fputs("unlocked", out) >= 0);
	CHECK(fflush_unlocked(NULL) == 0);
	CHECK(read(fds[0], text, sizeof(text)) == 8 && memcmp(text, "unlocked", 8) == 0);
	fclose(out);
	close(fds[0]);
}

// A catalog as gencat writes one: a header, a set, its messages and
// their texts.
static void write_catalog(const char *path, const unsigned char *bytes, size_t size) {
	int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
	CHECK(fd >= 0 && write(fd, bytes, size) == (ssize_t)size);
	close(fd);
}

static void be32(unsigned char *at, uint32_t value) {
	at[0] = value >> 24;
	at[1] = value >> 16;
	at[2] = value >> 8;
	at[3] = value;
}

// In musl's and the BSDs' format, which glibc does not read.
static void catalogs(void) {
	// One set, 1, with one message, 1: "hello".
	unsigned char good[50] = {0};
	be32(good, 0xff88ff89);
	be32(good + 4, 1);
	be32(good + 8, 30);
	be32(good + 12, 12);
	be32(good + 16, 24);
	be32(good + 20, 1);
	be32(good + 24, 1);
	be32(good + 28, 0);
	be32(good + 32, 1);
	be32(good + 36, 5);
	be32(good + 40, 0);
	memcpy(good + 44, "hello", 6);
	const char *path = "/tmp/library.cat";
	write_catalog(path, good, sizeof(good));
	nl_catd catd = catopen(path, 0);
	CHECK(catd != (nl_catd)-1);
	if(catd != (nl_catd)-1) {
		CHECK(strcmp(catgets(catd, 1, 1, "none"), "hello") == 0);
		CHECK(strcmp(catgets(catd, 1, 2, "none"), "none") == 0);
		CHECK(strcmp(catgets(catd, 2, 1, "none"), "none") == 0);
		CHECK(catclose(catd) == 0);
	}
	// A corrupt catalog has no messages, and is never read past its end:
	// a set whose messages start past it, a message whose text does, and
	// a last text with no end.
	unsigned char bad[50];
	static const struct {
		size_t at;
		uint32_t value;
	} corruptions[] = {{28, 1000000}, {24, 1000000}, {40, 1000000}, {40, 3}};
	for(size_t i = 0; i < sizeof(corruptions) / sizeof(corruptions[0]); i++) {
		memcpy(bad, good, sizeof(bad));
		be32(bad + corruptions[i].at, corruptions[i].value);
		if(corruptions[i].value == 3)
			bad[49] = '!';
		write_catalog(path, bad, sizeof(bad));
		catd = catopen(path, 0);
		CHECK(catd != (nl_catd)-1);
		if(catd != (nl_catd)-1) {
			CHECK(strcmp(catgets(catd, 1, 1, "none"), "none") == 0);
			catclose(catd);
		}
	}
	// Tables that do not fit the file, or a size that is not its own,
	// make no catalog.
	static const struct {
		size_t at;
		uint32_t value;
	} headers[] = {{4, 0x10000000}, {8, 31}, {12, 1000}, {16, 1000}};
	for(size_t i = 0; i < sizeof(headers) / sizeof(headers[0]); i++) {
		memcpy(bad, good, sizeof(bad));
		be32(bad + headers[i].at, headers[i].value);
		write_catalog(path, bad, sizeof(bad));
		CHECK(catopen(path, 0) == (nl_catd)-1);
	}
	write_catalog(path, good, 10);
	CHECK(catopen(path, 0) == (nl_catd)-1);
	unlink(path);
}

static void finished(void) {
	// Names: Linux's domain of a machine that has none, and ether_ntoa's
	// lower case without leading zeros.
	struct utsname name;
	CHECK(uname(&name) == 0 && strcmp(name.domainname, "(none)") == 0);
	char domain[64];
	CHECK(getdomainname(domain, sizeof(domain)) == 0 && strcmp(domain, "(none)") == 0);
	CHECK_ERROR(setdomainname("example", 7), EPERM);
	struct ether_addr address = {{0x00, 0x1a, 0x2b, 0x0c, 0xff, 0x01}};
	CHECK(strcmp(ether_ntoa(&address), "0:1a:2b:c:ff:1") == 0);
	struct ether_addr *read_back = ether_aton("0:1A:2b:c:ff:01");
	CHECK(read_back && memcmp(read_back, &address, sizeof(address)) == 0);
	CHECK(ether_aton("0:1a:2b:c:ff") == NULL);

	// tmpnam gives a name nothing has; tempnam takes the directory and
	// prefix asked for.
	char buffer[L_tmpnam];
	char *fresh = tmpnam(buffer);
	struct stat st;
	CHECK(fresh == buffer && strncmp(fresh, P_tmpdir "/", strlen(P_tmpdir) + 1) == 0);
	CHECK_ERROR(stat(fresh, &st), ENOENT);
	char *saved = getenv("TMPDIR") ? strdup(getenv("TMPDIR")) : NULL;
	unsetenv("TMPDIR");
	char *named = tempnam("/tmp", "lib");
	CHECK(named && strncmp(named, "/tmp/lib", 8) == 0);
	free(named);
	named = tempnam("/no/such/directory", "toolongprefix");
	CHECK(named && strncmp(named, P_tmpdir "/toolo", strlen(P_tmpdir) + 6) == 0);
	free(named);
	if(saved) {
		setenv("TMPDIR", saved, 1);
		free(saved);
	}
}

static void bsd_lines(void) {
	// fgetln: each line with its newline, then what is left.
	FILE *f = fmemopen("one\ntwo", 7, "r");
	CHECK(f != NULL);
	if(f) {
		size_t length;
		char *line = fgetln(f, &length);
		CHECK(line && length == 4 && memcmp(line, "one\n", 4) == 0);
		line = fgetln(f, &length);
		CHECK(line && length == 3 && memcmp(line, "two", 3) == 0);
		CHECK(fgetln(f, &length) == NULL);
		fclose(f);
	}
}

static void *stack_user(void *argument) {
	// Formatting a double uses aligned vector stores on x86-64, which a
	// stack misaligned by its creator breaks.
	char text[32];
	snprintf(text, sizeof(text), "%.2f", *(double *)argument);
	return strcmp(text, "1.50") == 0 ? argument : NULL;
}

static void stacks(void) {
	// A stack of the caller's whose end is not 16-byte aligned.
	enum { SIZE = 1 << 16 };
	static _Alignas(16) unsigned char memory[SIZE + 24];
	pthread_attr_t attr;
	CHECK(pthread_attr_init(&attr) == 0);
	CHECK(pthread_attr_setstack(&attr, memory + 8, SIZE) == 0);
	double value = 1.5;
	pthread_t thread;
	void *result = NULL;
	CHECK(pthread_create(&thread, &attr, stack_user, &value) == 0);
	CHECK(pthread_join(thread, &result) == 0 && result == &value);
	pthread_attr_destroy(&attr);
	// Sizes that are not whole pages.
	CHECK(pthread_attr_init(&attr) == 0);
	CHECK(pthread_attr_setstacksize(&attr, PTHREAD_STACK_MIN + 100) == 0);
	CHECK(pthread_attr_setguardsize(&attr, 100) == 0);
	CHECK(pthread_create(&thread, &attr, stack_user, &value) == 0);
	CHECK(pthread_join(thread, &result) == 0 && result == &value);
	pthread_attr_destroy(&attr);
}

static sigset_t thread_mask;

static void *report_mask(void *argument) {
	(void)argument;
	pthread_sigmask(SIG_BLOCK, NULL, &thread_mask);
	return NULL;
}

static int ends[2];

static void *read_forever(void *argument) {
	(void)argument;
	char byte;
	// read is a cancellation point.
	ssize_t n = read(ends[0], &byte, 1);
	(void)n;
	return NULL;
}

static void *read_all_blocked(void *argument) {
	// Every bit set by hand, as some programs make "every signal": the C
	// library's own signals stay as they were.
	sigset_t all;
	memset(&all, 0xff, sizeof(all));
	pthread_sigmask(SIG_SETMASK, &all, NULL);
	return read_forever(argument);
}

static void masks(void) {
	// A new thread has its creator's mask.
	pthread_t thread;
	sigset_t blocked;
	sigemptyset(&blocked);
	sigaddset(&blocked, SIGUSR2);
	pthread_sigmask(SIG_BLOCK, &blocked, NULL);
	CHECK(pthread_create(&thread, NULL, report_mask, NULL) == 0);
	CHECK(pthread_join(thread, NULL) == 0);
	CHECK(sigismember(&thread_mask, SIGUSR2) == 1);
	// And no more than that: it starts with every signal blocked and
	// takes that mask once it is set up.
	CHECK(sigismember(&thread_mask, SIGUSR1) == 0);
	pthread_sigmask(SIG_UNBLOCK, &blocked, NULL);

	// A thread made by one whose cancellation is disabled can be
	// cancelled in a wait.
	CHECK(pipe(ends) == 0);
	int old;
	CHECK(pthread_setcancelstate(PTHREAD_CANCEL_DISABLE, &old) == 0);
	CHECK(pthread_create(&thread, NULL, read_forever, NULL) == 0);
	CHECK(pthread_setcancelstate(old, NULL) == 0);
	usleep(20000);
	CHECK(pthread_cancel(thread) == 0);
	void *result = NULL;
	CHECK(pthread_join(thread, &result) == 0 && result == PTHREAD_CANCELED);
	// So can one that blocked every signal it could.
	CHECK(pthread_create(&thread, NULL, read_all_blocked, NULL) == 0);
	usleep(20000);
	CHECK(pthread_cancel(thread) == 0);
	CHECK(pthread_join(thread, &result) == 0 && result == PTHREAD_CANCELED);
	close(ends[0]);
	close(ends[1]);
}

static void found_by_python(void) {
	// sysconf answers what Python's multiprocessing and os.ttyname ask.
	CHECK(sysconf(_SC_SEM_NSEMS_MAX) >= 256);
	CHECK(sysconf(_SC_SEM_VALUE_MAX) >= 32767);
	CHECK(sysconf(_SC_TTY_NAME_MAX) == 32);

	// writev and readv of no buffers move nothing, without waiting, and
	// check the descriptor.
	int pipes[2];
	CHECK(pipe(pipes) == 0);
	struct iovec none = {NULL, 0};
	CHECK(writev(pipes[1], &none, 0) == 0);
	CHECK(writev(pipes[1], &none, 1) == 0);
	CHECK(readv(pipes[0], &none, 0) == 0);
	close(pipes[0]);
	close(pipes[1]);
	CHECK_ERROR(writev(pipes[1], &none, 0), EBADF);

	// strsignal's words are glibc's.
	CHECK(strcmp(strsignal(SIGINT), "Interrupt") == 0);
	CHECK(strcmp(strsignal(SIGKILL), "Killed") == 0);
	CHECK(strcmp(strsignal(SIGRTMIN + 3), "Real-time signal 3") == 0);
	CHECK(strcmp(strsignal(200), "Unknown signal 200") == 0);

	// pathconf and fpathconf need the file to be there.
	CHECK(pathconf("/", _PC_NAME_MAX) == 255);
	CHECK_ERROR(pathconf("/no/such/file", _PC_NAME_MAX), ENOENT);
	CHECK_ERROR(fpathconf(-1, _PC_NAME_MAX), EBADF);
	CHECK_ERROR(fpathconf(pipes[0], _PC_NAME_MAX), EBADF);

	// A mask of 0 leaves the mask as it is.
	int mask = LOG_UPTO(LOG_WARNING);
	int old = setlogmask(mask);
	CHECK(setlogmask(0) == mask);
	CHECK(setlogmask(old) == mask);

	// A product that rounds to zero keeps its sign.
	volatile double tiny = 1e-300, zero = 0.0;
	CHECK(signbit(fma(tiny, -tiny, zero)) && fma(tiny, -tiny, zero) == 0.0);
	CHECK(!signbit(fma(tiny, tiny, -zero)));

	// dladdr finds the static program itself, with no symbol.
	Dl_info info;
	CHECK(dladdr((void *)found_by_python, &info) != 0 && info.dli_fname
			&& strstr(info.dli_fname, "library") && info.dli_sname == NULL);

	// getaddrinfo: a service nobody names, and IPv6 scopes.
	struct addrinfo hints = {0}, *result;
	hints.ai_socktype = SOCK_STREAM;
	CHECK(getaddrinfo("127.0.0.1", "no-such-service", &hints, &result) == EAI_SERVICE);
	CHECK(getaddrinfo("127.0.0.1", "http", &hints, &result) == 0
			&& ntohs(((struct sockaddr_in *)result->ai_addr)->sin_port) == 80);
	if(result)
		freeaddrinfo(result);
	hints.ai_family = AF_INET6;
	result = NULL;
	CHECK(getaddrinfo("fe80::1%7", "80", &hints, &result) == 0
			&& ((struct sockaddr_in6 *)result->ai_addr)->sin6_scope_id == 7);
	if(result)
		freeaddrinfo(result);
	struct if_nameindex *interfaces = if_nameindex();
	CHECK(interfaces && interfaces[0].if_name);
	if(interfaces && interfaces[0].if_name) {
		char name[64];
		snprintf(name, sizeof(name), "ff02::1%%%s", interfaces[0].if_name);
		result = NULL;
		CHECK(getaddrinfo(name, "80", &hints, &result) == 0
				&& ((struct sockaddr_in6 *)result->ai_addr)->sin6_scope_id
						== interfaces[0].if_index);
		if(result)
			freeaddrinfo(result);
		// A global address takes a number only.
		snprintf(name, sizeof(name), "2001:db8::1%%%s", interfaces[0].if_name);
		CHECK(getaddrinfo(name, "80", &hints, &result) == EAI_NONAME);
		if_freenameindex(interfaces);
	}
	CHECK(getaddrinfo("fe80::1%no-such-interface", "80", &hints, &result) == EAI_NONAME);
}

int main(void) {
	check_start("library");
	RUN(allocations);
	RUN(shrinks);
	RUN(kept_blocks);
	RUN(thread_caches);
	RUN(environment);
	RUN(popens);
	RUN(flush_all);
	RUN(catalogs);
	RUN(finished);
	RUN(bsd_lines);
	RUN(stacks);
	RUN(masks);
	RUN(found_by_python);
	return check_end();
}
