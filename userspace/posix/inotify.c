// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// inotify (ADR 0615): the events of what is made, removed, moved, read,
// written and closed in a watched directory or of a watched file, in the
// order and with the names and cookies Linux gives; the watch calls and
// their errors; the queue's merges and its overflow; and what a read of a
// watch file gives and waits for. Uses /tmp.

#include <fcntl.h>
#include <poll.h>
#include <sys/ioctl.h>
#include <sys/inotify.h>
#include <sys/stat.h>
#include <sys/time.h>

#include "check.h"

#ifndef IN_MASK_CREATE
#define IN_MASK_CREATE 0x10000000
#endif

static const char *const DIR = "/tmp/inotify-test";
// The most events the queue holds, as Linux's default (and the kernel's).
#define QUEUE_MAX 16384

struct ev {
	int wd;
	uint32_t mask;
	uint32_t cookie;
	uint32_t len;
	char name[64];
};

// A path in the test's folder.
static const char *in_dir(const char *name) {
	static char path[4][128];
	static int next;
	char *out = path[next++ % 4];
	snprintf(out, 128, "%s/%s", DIR, name);
	return out;
}

static int watch_file(int flags) {
	int fd = inotify_init1(flags | IN_NONBLOCK);
	CHECK(fd >= 0);
	return fd;
}

static int add(int ifd, const char *path, uint32_t mask) {
	int wd = inotify_add_watch(ifd, path, mask);
	CHECK(wd >= 0);
	return wd;
}

static void make(const char *name, const char *text) {
	int fd = open(in_dir(name), O_WRONLY | O_CREAT | O_TRUNC, 0644);
	CHECK(fd >= 0);
	if(text)
		CHECK(write(fd, text, strlen(text)) == (ssize_t)strlen(text));
	close(fd);
}

// Reads every event the watch file holds into `out`, up to `max`; returns
// how many. Each record is whole, its length a multiple of 16 with its
// name.
static int drain(int ifd, struct ev *out, int max) {
	static char buf[65536];
	int count = 0;
	for(;;) {
		ssize_t got = read(ifd, buf, sizeof buf);
		if(got < 0) {
			CHECK(errno == EAGAIN);
			return count;
		}
		CHECK(got > 0);
		ssize_t at = 0;
		while(at < got) {
			struct inotify_event *e = (struct inotify_event *)(buf + at);
			CHECK(at + (ssize_t)sizeof *e + (ssize_t)e->len <= got);
			CHECK(e->len % 16 == 0);
			if(count < max) {
				struct ev *o = &out[count];
				o->wd = e->wd;
				o->mask = e->mask;
				o->cookie = e->cookie;
				o->len = e->len;
				snprintf(o->name, sizeof o->name, "%s", e->len ? e->name : "");
			}
			count++;
			at += (ssize_t)sizeof *e + (ssize_t)e->len;
		}
	}
}

// Checks that event `i` of `n` is `mask` of watch `wd` with `name`.
#define EVENT(evs, n, i, wd_, mask_, name_) \
	CHECK((i) < (n) && (evs)[i].wd == (wd_) && (evs)[i].mask == (uint32_t)(mask_) \
			&& !strcmp((evs)[i].name, (name_)))

// A queue that holds nothing.
static void empty(int ifd) {
	struct ev evs[8];
	int n = drain(ifd, evs, 8);
	if(n)
		printf("%s: %d events are left: first %#x of %d %s\n", check_name, n, evs[0].mask,
				evs[0].wd, evs[0].name);
	CHECK(n == 0);
}

static int pending(int ifd) {
	int bytes = -1;
	CHECK(ioctl(ifd, FIONREAD, &bytes) == 0);
	return bytes;
}

static void setup(void) {
	// Whatever an earlier run left.
	char command[160];
	snprintf(command, sizeof command, "rm -rf %s", DIR);
	CHECK(system(command) == 0);
	CHECK(mkdir(DIR, 0755) == 0);
}

static void entries_of_a_directory(void) {
	int ifd = watch_file(0);
	int wd = add(ifd, DIR, IN_ALL_EVENTS);
	struct ev evs[16];
	int n;

	// A file made by open: its name is made, and then it is opened.
	int fd = open(in_dir("a"), O_RDWR | O_CREAT, 0644);
	CHECK(fd >= 0);
	n = drain(ifd, evs, 16);
	CHECK(n == 2);
	EVENT(evs, n, 0, wd, IN_CREATE, "a");
	EVENT(evs, n, 1, wd, IN_OPEN, "a");
	// A name's record is the 16 bytes and the name with its NUL, to a
	// multiple of 16.
	CHECK(evs[0].len == 16);

	CHECK(write(fd, "hello", 5) == 5);
	CHECK(lseek(fd, 0, SEEK_SET) == 0);
	char byte[8];
	CHECK(read(fd, byte, sizeof byte) == 5);
	n = drain(ifd, evs, 16);
	CHECK(n == 2);
	EVENT(evs, n, 0, wd, IN_MODIFY, "a");
	EVENT(evs, n, 1, wd, IN_ACCESS, "a");

	// The last close of a description that could write.
	close(fd);
	n = drain(ifd, evs, 16);
	CHECK(n == 1);
	EVENT(evs, n, 0, wd, IN_CLOSE_WRITE, "a");

	// And of one that could not.
	fd = open(in_dir("a"), O_RDONLY);
	CHECK(fd >= 0);
	close(fd);
	n = drain(ifd, evs, 16);
	CHECK(n == 2);
	EVENT(evs, n, 0, wd, IN_OPEN, "a");
	EVENT(evs, n, 1, wd, IN_CLOSE_NOWRITE, "a");

	// A directory: IN_ISDIR with its events.
	CHECK(mkdir(in_dir("d"), 0755) == 0);
	n = drain(ifd, evs, 16);
	CHECK(n == 1);
	EVENT(evs, n, 0, wd, IN_CREATE | IN_ISDIR, "d");
	CHECK(rmdir(in_dir("d")) == 0);
	n = drain(ifd, evs, 16);
	CHECK(n == 1);
	EVENT(evs, n, 0, wd, IN_DELETE | IN_ISDIR, "d");

	// Links and the nodes made by mknod and symlink.
	CHECK(symlink("a", in_dir("s")) == 0);
	CHECK(link(in_dir("a"), in_dir("h")) == 0);
	CHECK(mknod(in_dir("f"), S_IFIFO | 0644, 0) == 0);
	n = drain(ifd, evs, 16);
	CHECK(n >= 3);
	EVENT(evs, n, 0, wd, IN_CREATE, "s");
	int found = 0;
	for(int i = 0; i < n && i < 16; i++)
		found += evs[i].mask == IN_CREATE && !strcmp(evs[i].name, "h");
	CHECK(found == 1);
	found = 0;
	for(int i = 0; i < n && i < 16; i++)
		found += evs[i].mask == IN_CREATE && !strcmp(evs[i].name, "f");
	CHECK(found == 1);

	// Names that go.
	CHECK(unlink(in_dir("s")) == 0);
	CHECK(unlink(in_dir("h")) == 0);
	CHECK(unlink(in_dir("f")) == 0);
	n = drain(ifd, evs, 16);
	CHECK(n == 3);
	EVENT(evs, n, 0, wd, IN_DELETE, "s");
	EVENT(evs, n, 1, wd, IN_DELETE, "h");
	EVENT(evs, n, 2, wd, IN_DELETE, "f");

	// Opening for truncation opens, and modifies.
	fd = open(in_dir("a"), O_WRONLY | O_TRUNC);
	CHECK(fd >= 0);
	close(fd);
	n = drain(ifd, evs, 16);
	CHECK(n == 3);
	EVENT(evs, n, 0, wd, IN_OPEN, "a");
	EVENT(evs, n, 1, wd, IN_MODIFY, "a");
	EVENT(evs, n, 2, wd, IN_CLOSE_WRITE, "a");

	CHECK(unlink(in_dir("a")) == 0);
	close(ifd);
}

static void changes_of_a_file(void) {
	make("b", "x");
	int ifd = watch_file(0);
	int wd = add(ifd, in_dir("b"), IN_ALL_EVENTS);
	struct ev evs[16];
	int n;

	// A watch of a file gives events without names.
	CHECK(chmod(in_dir("b"), 0600) == 0);
	n = drain(ifd, evs, 16);
	CHECK(n == 1);
	EVENT(evs, n, 0, wd, IN_ATTRIB, "");
	CHECK(evs[0].len == 0);
	struct timeval times[2] = {{1000, 0}, {2000, 0}};
	CHECK(utimes(in_dir("b"), times) == 0);
	n = drain(ifd, evs, 16);
	CHECK(n == 1);
	EVENT(evs, n, 0, wd, IN_ATTRIB, "");
	CHECK(truncate(in_dir("b"), 0) == 0);
	n = drain(ifd, evs, 16);
	CHECK(n == 1);
	EVENT(evs, n, 0, wd, IN_MODIFY, "");

	// Equal events in a row are one.
	int fd = open(in_dir("b"), O_RDWR);
	CHECK(fd >= 0);
	n = drain(ifd, evs, 16);
	CHECK(n == 1);
	EVENT(evs, n, 0, wd, IN_OPEN, "");
	CHECK(write(fd, "1", 1) == 1);
	CHECK(write(fd, "2", 1) == 1);
	CHECK(write(fd, "3", 1) == 1);
	CHECK(pending(ifd) == 16);
	// Another event between two of one kind keeps both.
	CHECK(fchmod(fd, 0640) == 0);
	CHECK(write(fd, "4", 1) == 1);
	n = drain(ifd, evs, 16);
	CHECK(n == 3);
	EVENT(evs, n, 0, wd, IN_MODIFY, "");
	EVENT(evs, n, 1, wd, IN_ATTRIB, "");
	EVENT(evs, n, 2, wd, IN_MODIFY, "");
	close(fd);
	n = drain(ifd, evs, 16);
	CHECK(n == 1);
	EVENT(evs, n, 0, wd, IN_CLOSE_WRITE, "");

	// The file's name going, with no descriptor of it open: its links
	// change, and then it is gone and the watch ends.
	CHECK(unlink(in_dir("b")) == 0);
	n = drain(ifd, evs, 16);
	CHECK(n == 3);
	EVENT(evs, n, 0, wd, IN_ATTRIB, "");
	EVENT(evs, n, 1, wd, IN_DELETE_SELF, "");
	EVENT(evs, n, 2, wd, IN_IGNORED, "");
	// The watch is gone: removing it again is EINVAL.
	CHECK_ERROR(inotify_rm_watch(ifd, wd), EINVAL);
	close(ifd);
}

static void moves(void) {
	CHECK(mkdir(in_dir("from"), 0755) == 0);
	CHECK(mkdir(in_dir("to"), 0755) == 0);
	make("from/m", "x");
	int ifd = watch_file(0);
	int from = add(ifd, in_dir("from"), IN_ALL_EVENTS);
	int to = add(ifd, in_dir("to"), IN_ALL_EVENTS);
	int self = add(ifd, in_dir("from/m"), IN_MOVE_SELF);
	struct ev evs[16];
	int n;

	// A move between directories: both halves with one cookie, and the
	// file told of its own move.
	CHECK(rename(in_dir("from/m"), in_dir("to/n")) == 0);
	n = drain(ifd, evs, 16);
	CHECK(n == 3);
	EVENT(evs, n, 0, from, IN_MOVED_FROM, "m");
	EVENT(evs, n, 1, to, IN_MOVED_TO, "n");
	EVENT(evs, n, 2, self, IN_MOVE_SELF, "");
	CHECK(evs[0].cookie != 0 && evs[0].cookie == evs[1].cookie);
	CHECK(evs[2].cookie == 0);
	uint32_t cookie = evs[0].cookie;

	// Another move has a cookie of its own.
	CHECK(rename(in_dir("to/n"), in_dir("to/o")) == 0);
	n = drain(ifd, evs, 16);
	CHECK(n == 3);
	EVENT(evs, n, 0, to, IN_MOVED_FROM, "n");
	EVENT(evs, n, 1, to, IN_MOVED_TO, "o");
	EVENT(evs, n, 2, self, IN_MOVE_SELF, "");
	CHECK(evs[0].cookie != 0 && evs[0].cookie == evs[1].cookie && evs[0].cookie != cookie);

	// A directory moved is told with IN_ISDIR.
	CHECK(mkdir(in_dir("from/sub"), 0755) == 0);
	n = drain(ifd, evs, 16);
	CHECK(n == 1);
	EVENT(evs, n, 0, from, IN_CREATE | IN_ISDIR, "sub");
	CHECK(rename(in_dir("from/sub"), in_dir("to/sub")) == 0);
	n = drain(ifd, evs, 16);
	CHECK(n == 2);
	EVENT(evs, n, 0, from, IN_MOVED_FROM | IN_ISDIR, "sub");
	EVENT(evs, n, 1, to, IN_MOVED_TO | IN_ISDIR, "sub");
	close(ifd);
}

static void removed_directory(void) {
	CHECK(mkdir(in_dir("gone"), 0755) == 0);
	int ifd = watch_file(0);
	int wd = add(ifd, in_dir("gone"), IN_ALL_EVENTS);
	struct ev evs[8];
	CHECK(rmdir(in_dir("gone")) == 0);
	int n = drain(ifd, evs, 8);
	CHECK(n == 2);
	EVENT(evs, n, 0, wd, IN_DELETE_SELF, "");
	EVENT(evs, n, 1, wd, IN_IGNORED, "");
	close(ifd);
}

static void masks_and_watch_numbers(void) {
	make("c", NULL);
	CHECK(symlink("c", in_dir("link")) == 0);
	int ifd = watch_file(0);
	struct ev evs[8];
	int n;

	int wd = add(ifd, in_dir("c"), IN_MODIFY);
	// The same file again keeps its number, and a mask replaces the old.
	CHECK(inotify_add_watch(ifd, in_dir("c"), IN_ATTRIB) == wd);
	CHECK(chmod(in_dir("c"), 0600) == 0);
	CHECK(truncate(in_dir("c"), 0) == 0);
	n = drain(ifd, evs, 8);
	CHECK(n == 1);
	EVENT(evs, n, 0, wd, IN_ATTRIB, "");
	// IN_MASK_ADD takes the new bits as well.
	CHECK(inotify_add_watch(ifd, in_dir("c"), IN_MODIFY | IN_MASK_ADD) == wd);
	CHECK(chmod(in_dir("c"), 0640) == 0);
	CHECK(truncate(in_dir("c"), 0) == 0);
	n = drain(ifd, evs, 8);
	CHECK(n == 2);
	EVENT(evs, n, 0, wd, IN_ATTRIB, "");
	EVENT(evs, n, 1, wd, IN_MODIFY, "");
	// The same file by a link: the file's own watch.
	CHECK(inotify_add_watch(ifd, in_dir("link"), IN_ATTRIB) == wd);
	// With IN_DONT_FOLLOW, the link's.
	int link_wd = add(ifd, in_dir("link"), IN_ATTRIB | IN_DONT_FOLLOW);
	CHECK(link_wd != wd);
	CHECK(inotify_add_watch(ifd, in_dir("link"), IN_ATTRIB | IN_DONT_FOLLOW) == link_wd);
	// A watch is made once with IN_MASK_CREATE.
	CHECK_ERROR(inotify_add_watch(ifd, in_dir("c"), IN_ATTRIB | IN_MASK_CREATE), EEXIST);
	CHECK(inotify_add_watch(ifd, in_dir("link"), IN_MASK_CREATE | IN_ATTRIB) == -1);
	// IN_ONLYDIR names directories alone.
	CHECK_ERROR(inotify_add_watch(ifd, in_dir("c"), IN_ATTRIB | IN_ONLYDIR), ENOTDIR);
	int dir_wd = inotify_add_watch(ifd, DIR, IN_ATTRIB | IN_ONLYDIR);
	CHECK(dir_wd >= 0 && dir_wd != wd && dir_wd != link_wd);
	CHECK(inotify_rm_watch(ifd, dir_wd) == 0);
	n = drain(ifd, evs, 8);
	CHECK(n == 1);
	EVENT(evs, n, 0, dir_wd, IN_IGNORED, "");

	// What fails: no event in the mask, no such file, a bad address.
	CHECK_ERROR(inotify_add_watch(ifd, in_dir("c"), 0), EINVAL);
	CHECK_ERROR(inotify_add_watch(ifd, in_dir("c"), IN_MASK_ADD | IN_MASK_CREATE | IN_ATTRIB),
			EINVAL);
	CHECK_ERROR(inotify_add_watch(ifd, in_dir("absent"), IN_ATTRIB), ENOENT);
	CHECK_ERROR(inotify_add_watch(ifd, in_dir("c/below"), IN_ATTRIB), ENOTDIR);
	CHECK_ERROR(inotify_add_watch(ifd, NULL, IN_ATTRIB), EFAULT);
	CHECK_ERROR(inotify_add_watch(-1, in_dir("c"), IN_ATTRIB), EBADF);
	// A descriptor that is no watch file.
	int p[2];
	CHECK(pipe(p) == 0);
	CHECK_ERROR(inotify_add_watch(p[0], in_dir("c"), IN_ATTRIB), EINVAL);
	CHECK_ERROR(inotify_rm_watch(p[0], wd), EINVAL);
	CHECK_ERROR(inotify_rm_watch(-1, wd), EBADF);
	close(p[0]);
	close(p[1]);
	// A watch that was never made.
	CHECK_ERROR(inotify_rm_watch(ifd, 12345), EINVAL);

	// Removing a watch says it is ignored, and ends its events.
	empty(ifd);
	CHECK(inotify_rm_watch(ifd, wd) == 0);
	n = drain(ifd, evs, 8);
	CHECK(n == 1);
	EVENT(evs, n, 0, wd, IN_IGNORED, "");
	CHECK(chmod(in_dir("c"), 0600) == 0);
	empty(ifd);
	CHECK_ERROR(inotify_rm_watch(ifd, wd), EINVAL);
	close(ifd);
}

static void one_shot(void) {
	make("e", NULL);
	int ifd = watch_file(0);
	int wd = add(ifd, in_dir("e"), IN_MODIFY | IN_ONESHOT);
	struct ev evs[8];
	CHECK(truncate(in_dir("e"), 1) == 0);
	CHECK(truncate(in_dir("e"), 2) == 0);
	int n = drain(ifd, evs, 8);
	CHECK(n == 2);
	EVENT(evs, n, 0, wd, IN_MODIFY, "");
	EVENT(evs, n, 1, wd, IN_IGNORED, "");
	// The watch is gone with its event.
	CHECK_ERROR(inotify_rm_watch(ifd, wd), EINVAL);
	close(ifd);
}

static void unlinked_while_open(void) {
	make("g", NULL);
	int a = watch_file(0), b = watch_file(0);
	int with = add(a, DIR, IN_MODIFY | IN_EXCL_UNLINK);
	int without = add(b, DIR, IN_MODIFY);
	int fd = open(in_dir("g"), O_WRONLY);
	CHECK(fd >= 0);
	struct ev evs[8];
	CHECK(write(fd, "1", 1) == 1);
	int n = drain(a, evs, 8);
	CHECK(n == 1);
	EVENT(evs, n, 0, with, IN_MODIFY, "g");
	n = drain(b, evs, 8);
	CHECK(n == 1);
	EVENT(evs, n, 0, without, IN_MODIFY, "g");
	// Once the file has no name, the watch that excludes it hears no more.
	CHECK(unlink(in_dir("g")) == 0);
	CHECK(write(fd, "2", 1) == 1);
	empty(a);
	n = drain(b, evs, 8);
	CHECK(n == 1);
	EVENT(evs, n, 0, without, IN_MODIFY, "g");
	close(fd);
	close(a);
	close(b);
}

static void reading(void) {
	make("h", NULL);
	int ifd = watch_file(0);
	int wd = add(ifd, DIR, IN_CREATE | IN_MODIFY);
	char buf[256];

	// Nothing waits.
	CHECK_ERROR(read(ifd, buf, sizeof buf), EAGAIN);
	CHECK(pending(ifd) == 0);
	struct pollfd pfd = {ifd, POLLIN, 0};
	CHECK(poll(&pfd, 1, 0) == 0);

	make("long-enough-a-name-to-need-padding", NULL);
	make("x", NULL);
	// IN_CREATE for each, and IN_MODIFY for none: they were not cut.
	// The name's record is 16 bytes and 48 for the name and its NUL.
	CHECK(pending(ifd) == (16 + 48) + (16 + 16));
	pfd.revents = 0;
	CHECK(poll(&pfd, 1, 0) == 1 && (pfd.revents & POLLIN));

	// A buffer that cannot hold the first record: EINVAL, and nothing is
	// taken.
	CHECK_ERROR(read(ifd, buf, 15), EINVAL);
	CHECK_ERROR(read(ifd, buf, 16), EINVAL);
	CHECK_ERROR(read(ifd, buf, 63), EINVAL);
	CHECK(pending(ifd) == (16 + 48) + (16 + 16));
	// One that holds the first alone gives it, whole.
	CHECK(read(ifd, buf, 64) == 64);
	struct inotify_event *e = (struct inotify_event *)buf;
	CHECK(e->wd == wd && e->mask == IN_CREATE && e->len == 48);
	CHECK(!strcmp(e->name, "long-enough-a-name-to-need-padding"));
	// The padding after the name is NULs.
	for(int i = (int)strlen(e->name); i < 48; i++)
		CHECK(e->name[i] == 0);
	CHECK(pending(ifd) == 32);
	// A buffer that has room for a part of the next: the whole ones.
	CHECK(read(ifd, buf, 40) == 32);
	CHECK(!strcmp(((struct inotify_event *)buf)->name, "x"));
	CHECK_ERROR(read(ifd, buf, sizeof buf), EAGAIN);
	pfd.revents = 0;
	CHECK(poll(&pfd, 1, 0) == 0);
	close(ifd);
}

static void waiting(void) {
	// A watch file that blocks waits for an event that comes later.
	int ifd = inotify_init1(0);
	CHECK(ifd >= 0);
	CHECK((fcntl(ifd, F_GETFL) & O_NONBLOCK) == 0);
	int wd = add(ifd, DIR, IN_CREATE);
	pid_t child = check_fork();
	if(child == 0) {
		check_sleep_ms(100);
		make("late", NULL);
		check_exit_child();
	}
	long long start = check_now_ns();
	char buf[64];
	ssize_t got = read(ifd, buf, sizeof buf);
	long long waited = check_now_ns() - start;
	CHECK(got == 16 + 16);
	CHECK(waited > 50 * 1000000LL);
	struct inotify_event *e = (struct inotify_event *)buf;
	CHECK(got == 32 && e->wd == wd && e->mask == IN_CREATE && !strcmp(e->name, "late"));
	CHECK_CHILD(child);

	// A poll waits too, and a time-out ends it.
	struct pollfd pfd = {ifd, POLLIN, 0};
	CHECK(poll(&pfd, 1, 50) == 0);
	child = check_fork();
	if(child == 0) {
		check_sleep_ms(100);
		CHECK(unlink(in_dir("late")) == 0);
		make("later", NULL);
		check_exit_child();
	}
	CHECK(poll(&pfd, 1, 5000) == 1 && (pfd.revents & POLLIN));
	CHECK_CHILD(child);
	close(ifd);

	// IN_CLOEXEC closes it on exec, and IN_NONBLOCK makes it not wait.
	ifd = inotify_init1(IN_CLOEXEC | IN_NONBLOCK);
	CHECK(ifd >= 0);
	CHECK(fcntl(ifd, F_GETFD) & FD_CLOEXEC);
	CHECK(fcntl(ifd, F_GETFL) & O_NONBLOCK);
	close(ifd);
	ifd = inotify_init();
	CHECK(ifd >= 0);
	CHECK(!(fcntl(ifd, F_GETFD) & FD_CLOEXEC));
	close(ifd);
	// Flags there are none of.
	CHECK_ERROR(inotify_init1(1 << 30), EINVAL);
	// Writes are for no one.
	ifd = watch_file(0);
	CHECK_ERROR(write(ifd, "x", 1), EBADF);
	close(ifd);
}

static void overflowing(void) {
	make("o1", NULL);
	make("o2", NULL);
	int ifd = watch_file(0);
	int wd = add(ifd, DIR, IN_MODIFY);
	int a = open(in_dir("o1"), O_WRONLY);
	int b = open(in_dir("o2"), O_WRONLY);
	CHECK(a >= 0 && b >= 0);
	// Alternating names are never equal, so that none merge.
	for(int i = 0; i < QUEUE_MAX / 2 + 100; i++) {
		CHECK(write(a, "x", 1) == 1);
		CHECK(write(b, "x", 1) == 1);
	}
	CHECK(pending(ifd) == QUEUE_MAX * 32 + 16);
	// Every event that fit, and then one that says the rest did not,
	// for no watch.
	static struct ev evs[QUEUE_MAX + 8];
	int n = drain(ifd, evs, QUEUE_MAX + 8);
	CHECK(n == QUEUE_MAX + 1);
	EVENT(evs, n, 0, wd, IN_MODIFY, "o1");
	EVENT(evs, n, 1, wd, IN_MODIFY, "o2");
	EVENT(evs, n, QUEUE_MAX - 1, wd, IN_MODIFY, "o2");
	EVENT(evs, n, QUEUE_MAX, -1, IN_Q_OVERFLOW, "");
	// Drained, it takes events again.
	CHECK(write(a, "x", 1) == 1);
	n = drain(ifd, evs, 8);
	CHECK(n == 1);
	EVENT(evs, n, 0, wd, IN_MODIFY, "o1");
	close(a);
	close(b);
	close(ifd);
}

static void closing_the_watch_file(void) {
	// A watch file closed ends its watches: the files are free to go.
	make("k", NULL);
	int ifd = watch_file(0);
	add(ifd, in_dir("k"), IN_ALL_EVENTS);
	close(ifd);
	CHECK(unlink(in_dir("k")) == 0);
	// A copy of its descriptor keeps it.
	make("k", NULL);
	ifd = watch_file(0);
	int wd = add(ifd, in_dir("k"), IN_ATTRIB);
	int copy = dup(ifd);
	CHECK(copy >= 0);
	close(ifd);
	CHECK(chmod(in_dir("k"), 0600) == 0);
	struct ev evs[4];
	int n = drain(copy, evs, 4);
	CHECK(n == 1);
	EVENT(evs, n, 0, wd, IN_ATTRIB, "");
	close(copy);
}

int main(void) {
	check_start("inotify");
	setup();
	RUN(entries_of_a_directory);
	RUN(changes_of_a_file);
	RUN(moves);
	RUN(removed_directory);
	RUN(masks_and_watch_numbers);
	RUN(one_shot);
	RUN(unlinked_while_open);
	RUN(reading);
	RUN(waiting);
	RUN(overflowing);
	RUN(closing_the_watch_file);
	char command[160];
	snprintf(command, sizeof command, "rm -rf %s", DIR);
	if(system(command))
		CHECK(0);
	return check_end();
}
