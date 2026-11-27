// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Event queues, kqueue(2) and kevent(2) (ADR 0353): the descriptors a
// queue takes and refuses, a change's errors and receipts, pipes and
// sockets watched for reading and writing (level, edge, oneshot, dispatch
// and disabled watches, and what reads report), user events, threads and
// signals that end a wait, timeouts, what a watch holds across close,
// dup2 and fork, poll on a queue, and a thousand watches at once.

#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <pthread.h>
#include <radix/radix.h>
#include <stdint.h>
#include <sys/event.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>

#include "check.h"

// A wait that finds nothing: a time of 0.
static const struct timespec NO_WAIT = {0, 0};

// A wait for what is there or soon will be: five seconds at most, so that
// an event that is lost fails a check where it would hang the test.
static const struct timespec PATIENCE = {5, 0};

// Looks at the queue `kq` and writes up to `room` events at `events`,
// without waiting; returns how many.
static int look(int kq, struct kevent *events, int room) {
	return kevent(kq, NULL, 0, events, room, &NO_WAIT);
}

// As look, but waits for the first event for five seconds at most.
static int wait_for(int kq, struct kevent *events, int room) {
	return kevent(kq, NULL, 0, events, room, &PATIENCE);
}

// Makes one change of the queue `kq` and asks for no event: 0, or -1
// with errno if the change failed.
static int watch(int kq, uintptr_t ident, short filter, unsigned short flags, void *udata) {
	struct kevent change;
	EV_SET(&change, ident, filter, flags, 0, 0, udata);
	return kevent(kq, &change, 1, NULL, 0, NULL);
}

// Whether `event` is for `ident` and `filter`, has exactly `flags`, and
// carries no data and no fflags, as a read or write event does.
static int reports(const struct kevent *event, uintptr_t ident, short filter,
		unsigned short flags) {
	return event->ident == ident && event->filter == filter && event->flags == flags
			&& event->fflags == 0 && event->data == 0;
}

// The event of the `count` at `events` for `ident` and `filter`, or NULL.
static const struct kevent *find_event(const struct kevent *events, int count, uintptr_t ident,
		short filter) {
	for(int i = 0; i < count; i++)
		if(events[i].ident == ident && events[i].filter == filter)
			return &events[i];
	return NULL;
}

// Checks that the change of `ident`, `filter` and `flags` fails with
// `error`: as an event with EV_ERROR and the errno in its data when there
// is room, and as kevent's own failure when there is none.
static void change_fails(int kq, uintptr_t ident, short filter, unsigned short flags, int error) {
	struct kevent change, events[2] = {{0}};
	EV_SET(&change, ident, filter, flags, 0, 0, NULL);
	CHECK(kevent(kq, &change, 1, events, 2, &NO_WAIT) == 1);
	CHECK((events[0].flags & EV_ERROR) && events[0].data == error);
	CHECK(events[0].ident == ident && events[0].filter == filter);
	CHECK_ERROR(kevent(kq, &change, 1, NULL, 0, NULL), error);
}

// Waits up to ten seconds for the child `pid` to exit with 0. A child
// that has not ended by then is killed, and the check fails.
static void wait_for_child(pid_t pid) {
	int status = 0;
	pid_t got = 0;
	for(int waited = 0; waited < 1000 && got == 0; waited++) {
		got = waitpid(pid, &status, WNOHANG);
		if(got == 0)
			check_sleep_ms(10);
	}
	if(got == 0) {
		kill(pid, SIGKILL);
		waitpid(pid, &status, 0);
		check_failed(__FILE__, __LINE__, "the child ended within ten seconds", 0);
		return;
	}
	CHECK(got == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

// Joins `thread`, which sets `*finished` as its last act, once that is
// set: a thread that does not finish within five seconds is left, and the
// check fails.
static void join_within(pthread_t thread, int *finished) {
	for(int waited = 0; waited < 500 && !__atomic_load_n(finished, __ATOMIC_ACQUIRE); waited++)
		check_sleep_ms(10);
	int done = __atomic_load_n(finished, __ATOMIC_ACQUIRE);
	CHECK(done);
	if(done)
		CHECK(pthread_join(thread, NULL) == 0);
}

// Waits up to five seconds until `fd` can be read; returns whether it can.
static int readable_soon(int fd) {
	struct pollfd entry = {.fd = fd, .events = POLLIN};
	return poll(&entry, 1, 5000) == 1;
}

// Raises the soft limit on descriptors to the hard one, for the steps that
// open thousands.
static void raise_files_limit(void) {
	struct rlimit files;
	CHECK(getrlimit(RLIMIT_NOFILE, &files) == 0);
	files.rlim_cur = files.rlim_max;
	CHECK(setrlimit(RLIMIT_NOFILE, &files) == 0);
}

// A queue is a descriptor like any other, and kevent takes only a queue.
static void basics(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	CHECK(fcntl(kq, F_GETFD) == 0);
	CHECK(fcntl(kq, F_SETFD, FD_CLOEXEC) == 0);
	CHECK(fcntl(kq, F_GETFD) == FD_CLOEXEC);
	CHECK(kevent(kq, NULL, 0, NULL, 0, NULL) == 0);
	CHECK(close(kq) == 0);
	CHECK_ERROR(kevent(kq, NULL, 0, NULL, 0, NULL), EBADF);
	// A queue takes the lowest free descriptor, the one just closed.
	int again = kqueue();
	CHECK(again == kq);
	// A descriptor that is not a queue, or not open: the kernel's
	// wrong_kind, empty_slot and no_such_slot, which are EBADF.
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK_ERROR(kevent(fds[0], NULL, 0, NULL, 0, NULL), EBADF);
	CHECK_ERROR(kevent(-1, NULL, 0, NULL, 0, NULL), EBADF);
	CHECK_ERROR(kevent(1000, NULL, 0, NULL, 0, NULL), EBADF);
	close(fds[0]);
	close(fds[1]);
	close(again);
}

// The most changes one call takes, and one more, all passed over: a change
// of filter 0 does nothing.
static struct kevent passed_over[RADIX_EVENT_QUEUE_CHANGES_MAX + 1];

// What a change or a call may not do, and how each failure is told: the
// event of a change that failed, or kevent's own.
static void errors(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	struct kevent change, events[4] = {{0}};

	// A filter the queue does not have.
	change_fails(kq, fds[0], -7, EV_ADD, EINVAL);
	// A descriptor that is not open, closed or never open.
	int closed = dup(fds[0]);
	CHECK(closed >= 0);
	close(closed);
	change_fails(kq, closed, EVFILT_READ, EV_ADD, EBADF);
	change_fails(kq, 1000, EVFILT_WRITE, EV_ADD, EBADF);
	// Files and directories are always ready.
	int file = open("/tmp/events-file", O_RDWR | O_CREAT | O_TRUNC, 0644);
	CHECK(file >= 0);
	change_fails(kq, file, EVFILT_READ, EV_ADD, EPERM);
	change_fails(kq, file, EVFILT_WRITE, EV_ADD, EPERM);
	close(file);
	unlink("/tmp/events-file");
	int directory = open("/", O_RDONLY | O_DIRECTORY);
	CHECK(directory >= 0);
	change_fails(kq, directory, EVFILT_READ, EV_ADD, EPERM);
	close(directory);
	// A queue watching a queue, another or itself.
	int other = kqueue();
	CHECK(other >= 0);
	change_fails(kq, other, EVFILT_READ, EV_ADD, EINVAL);
	change_fails(kq, kq, EVFILT_WRITE, EV_ADD, EINVAL);
	close(other);
	// A watch the queue does not hold: deleted, or changed without EV_ADD.
	change_fails(kq, fds[0], EVFILT_READ, EV_DELETE, ENOENT);
	change_fails(kq, 5, EVFILT_USER, EV_DELETE, ENOENT);
	change_fails(kq, fds[0], EVFILT_READ, EV_ENABLE, ENOENT);
	change_fails(kq, fds[0], EVFILT_READ, EV_DISABLE, ENOENT);
	change_fails(kq, 5, EVFILT_USER, 0, ENOENT);
	// Adding and deleting at once.
	change_fails(kq, fds[0], EVFILT_READ, EV_ADD | EV_DELETE, EINVAL);

	// The counts and the time are checked before the queue is used.
	EV_SET(&change, fds[0], EVFILT_READ, EV_ADD, 0, 0, NULL);
	CHECK_ERROR(kevent(kq, &change, -1, NULL, 0, NULL), EINVAL);
	CHECK_ERROR(kevent(kq, NULL, 0, events, -1, &NO_WAIT), EINVAL);
	CHECK_ERROR(kevent(kq, passed_over, RADIX_EVENT_QUEUE_CHANGES_MAX + 1, NULL, 0, NULL),
			EINVAL);
	CHECK(kevent(kq, passed_over, RADIX_EVENT_QUEUE_CHANGES_MAX, NULL, 0, NULL) == 0);
	struct timespec bad = {.tv_sec = 0, .tv_nsec = 1000000000};
	CHECK_ERROR(kevent(kq, NULL, 0, events, 4, &bad), EINVAL);
	CHECK_ERROR(kevent(kq, NULL, 0, NULL, 0, &bad), EINVAL);
	bad.tv_nsec = -1;
	CHECK_ERROR(kevent(kq, NULL, 0, events, 4, &bad), EINVAL);
	bad.tv_sec = -1;
	bad.tv_nsec = 0;
	CHECK_ERROR(kevent(kq, NULL, 0, events, 4, &bad), EINVAL);
	// A list of changes the program cannot read.
	CHECK_ERROR(kevent(kq, NULL, 1, NULL, 0, NULL), EFAULT);
	CHECK_ERROR(kevent(kq, (const struct kevent *)(uintptr_t)16, 1, NULL, 0, NULL), EFAULT);

	// A change that fails with no room for its event fails the call, with
	// the changes before it made.
	CHECK(write(fds[1], "a", 1) == 1);
	struct kevent two[2];
	EV_SET(&two[0], fds[0], EVFILT_READ, EV_ADD, 0, 0, NULL);
	EV_SET(&two[1], fds[0], -7, EV_ADD, 0, 0, NULL);
	CHECK_ERROR(kevent(kq, two, 2, NULL, 0, NULL), EINVAL);
	CHECK(look(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[0], EVFILT_READ, 0));
	close(fds[0]);
	close(fds[1]);
	close(kq);
}

// EV_RECEIPT: each change says how it went, and a call that gave receipts
// reports no watch.
static void receipt(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int readable[2], idle[2];
	CHECK(pipe(readable) == 0 && pipe(idle) == 0);
	CHECK(write(readable[1], "r", 1) == 1);
	struct kevent changes[3], events[4] = {{0}};
	EV_SET(&changes[0], readable[0], EVFILT_READ, EV_ADD | EV_RECEIPT, 0, 0, NULL);
	EV_SET(&changes[1], idle[0], EVFILT_READ, EV_ADD | EV_RECEIPT, 0, 0, NULL);
	EV_SET(&changes[2], readable[0], -7, EV_ADD | EV_RECEIPT, 0, 0, NULL);
	// The receipts are the three events, in the order of the changes, and
	// the pipe that can be read is not reported by this call.
	CHECK(kevent(kq, changes, 3, events, 3, &NO_WAIT) == 3);
	for(int i = 0; i < 3; i++)
		CHECK(events[i].flags & EV_ERROR);
	CHECK(events[0].data == 0 && events[1].data == 0 && events[2].data == EINVAL);
	CHECK(events[0].ident == (uintptr_t)readable[0] && events[1].ident == (uintptr_t)idle[0]);
	CHECK(events[2].ident == (uintptr_t)readable[0] && events[2].filter == -7);
	// With room left over, still only the receipts.
	memset(events, 0, sizeof(events));
	CHECK(kevent(kq, changes, 3, events, 4, &NO_WAIT) == 3);
	CHECK(events[0].data == 0 && events[1].data == 0 && events[2].data == EINVAL);
	// The next call reports the watch.
	CHECK(look(kq, events, 4) == 1);
	CHECK(reports(&events[0], readable[0], EVFILT_READ, 0));
	close(kq);

	// Without room for a receipt, the call stops there and returns the
	// receipts written so far: one, or none.
	kq = kqueue();
	CHECK(kq >= 0);
	memset(events, 0, sizeof(events));
	CHECK(kevent(kq, changes, 2, events, 1, &NO_WAIT) == 1);
	CHECK((events[0].flags & EV_ERROR) && events[0].data == 0);
	CHECK(kevent(kq, changes, 1, NULL, 0, NULL) == 0);
	close(kq);
	close(readable[0]);
	close(readable[1]);
	close(idle[0]);
	close(idle[1]);
}

// A pipe's two ends, watched for reading and for writing: what each
// reports while it is level-triggered, and at the end of the pipe.
static void pipes(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	int marker;
	struct kevent events[4] = {{0}};
	CHECK(watch(kq, fds[0], EVFILT_READ, EV_ADD, &marker) == 0);
	CHECK(look(kq, events, 4) == 0);
	CHECK(write(fds[1], "x", 1) == 1);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[0], EVFILT_READ, 0) && events[0].udata == &marker);
	// A watch is level-triggered: reported again while the byte is unread.
	CHECK(look(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[0], EVFILT_READ, 0) && events[0].udata == &marker);
	char byte;
	CHECK(read(fds[0], &byte, 1) == 1);
	CHECK(look(kq, events, 4) == 0);

	// The write end can be written at once.
	CHECK(watch(kq, fds[1], EVFILT_WRITE, EV_ADD, NULL) == 0);
	CHECK(look(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[1], EVFILT_WRITE, 0));
	// Closing it ends the watch on it, and the read watch reports the end.
	close(fds[1]);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[0], EVFILT_READ, EV_EOF));
	close(fds[0]);

	// The write end of a pipe whose reader has gone.
	CHECK(pipe(fds) == 0);
	CHECK(watch(kq, fds[1], EVFILT_WRITE, EV_ADD, NULL) == 0);
	CHECK(look(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[1], EVFILT_WRITE, 0));
	close(fds[0]);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[1], EVFILT_WRITE, EV_EOF));
	close(fds[1]);
	close(kq);
}

// EV_CLEAR: a watch is reported when its pipe changes, not while it is
// ready.
static void edge(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	int first, second;
	struct kevent events[4] = {{0}};
	CHECK(watch(kq, fds[0], EVFILT_READ, EV_ADD | EV_CLEAR, &first) == 0);
	// Added again, it has the new udata and stays edge-triggered.
	CHECK(watch(kq, fds[0], EVFILT_READ, EV_ADD, &second) == 0);
	CHECK(look(kq, events, 4) == 0);
	CHECK(write(fds[1], "a", 1) == 1);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[0], EVFILT_READ, EV_CLEAR) && events[0].udata == &second);
	// The byte is unread, but nothing has changed since it was reported.
	CHECK(look(kq, events, 4) == 0);
	CHECK(write(fds[1], "b", 1) == 1);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[0], EVFILT_READ, EV_CLEAR));
	CHECK(look(kq, events, 4) == 0);
	close(fds[0]);
	close(fds[1]);
	close(kq);
}

// EV_CLEAR and reads (ADR 0777): a read that leaves bytes in the pipe is
// a change its read watch reports, one that takes the last byte leaves it
// nothing to report, and each makes room its write watch reports. Once
// the writer has gone, the read that takes the last byte reports the end.
static void edge_reads(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	struct kevent events[4] = {{0}};
	CHECK(watch(kq, fds[0], EVFILT_READ, EV_ADD | EV_CLEAR, NULL) == 0);
	CHECK(watch(kq, fds[1], EVFILT_WRITE, EV_ADD | EV_CLEAR, NULL) == 0);
	// The write end is ready as its watch is added, the read end not yet.
	CHECK(look(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[1], EVFILT_WRITE, EV_CLEAR));
	CHECK(write(fds[1], "ab", 2) == 2);
	CHECK(look(kq, events, 4) == 2);
	CHECK(find_event(events, 2, fds[0], EVFILT_READ) != NULL);
	CHECK(find_event(events, 2, fds[1], EVFILT_WRITE) != NULL);
	char byte;
	CHECK(read(fds[0], &byte, 1) == 1 && byte == 'a');
	CHECK(look(kq, events, 4) == 2);
	CHECK(find_event(events, 2, fds[0], EVFILT_READ) != NULL);
	CHECK(find_event(events, 2, fds[1], EVFILT_WRITE) != NULL);
	CHECK(read(fds[0], &byte, 1) == 1 && byte == 'b');
	CHECK(look(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[1], EVFILT_WRITE, EV_CLEAR));
	CHECK(look(kq, events, 4) == 0);
	// The next write reports the read end again.
	CHECK(write(fds[1], "c", 1) == 1);
	CHECK(look(kq, events, 4) == 2);
	CHECK(find_event(events, 2, fds[0], EVFILT_READ) != NULL);
	// The writer's going ends its watch, and the read watch reports the
	// end, and again once the last byte is read.
	close(fds[1]);
	CHECK(look(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[0], EVFILT_READ, EV_CLEAR | EV_EOF));
	CHECK(read(fds[0], &byte, 1) == 1 && byte == 'c');
	CHECK(look(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[0], EVFILT_READ, EV_CLEAR | EV_EOF));
	CHECK(look(kq, events, 4) == 0);
	close(fds[0]);
	close(kq);
}

// EV_ONESHOT deletes a watch once it is reported, EV_DISPATCH disables it.
static void oneshot_dispatch(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int once[2], dispatched[2];
	CHECK(pipe(once) == 0 && pipe(dispatched) == 0);
	CHECK(write(once[1], "o", 1) == 1 && write(dispatched[1], "d", 1) == 1);
	struct kevent events[4] = {{0}};

	// A oneshot watch is reported once, though the pipe is still
	// readable, and is deleted by that.
	CHECK(watch(kq, once[0], EVFILT_READ, EV_ADD | EV_ONESHOT, NULL) == 0);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], once[0], EVFILT_READ, EV_ONESHOT));
	CHECK(look(kq, events, 4) == 0);
	change_fails(kq, once[0], EVFILT_READ, EV_DELETE, ENOENT);

	// A dispatch watch is reported once and then disabled, but kept:
	// enabling it makes it report again.
	CHECK(watch(kq, dispatched[0], EVFILT_READ, EV_ADD | EV_DISPATCH, NULL) == 0);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], dispatched[0], EVFILT_READ, EV_DISPATCH));
	CHECK(look(kq, events, 4) == 0);
	CHECK(watch(kq, dispatched[0], EVFILT_READ, EV_ENABLE, NULL) == 0);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], dispatched[0], EVFILT_READ, EV_DISPATCH));
	CHECK(look(kq, events, 4) == 0);
	CHECK(watch(kq, dispatched[0], EVFILT_READ, EV_DELETE, NULL) == 0);
	close(once[0]);
	close(once[1]);
	close(dispatched[0]);
	close(dispatched[1]);
	close(kq);
}

// A disabled watch is not reported until it is enabled.
static void disable(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(write(fds[1], "x", 1) == 1);
	int first, second;
	struct kevent events[4] = {{0}};
	// Added disabled, a watch is not reported while the pipe is readable.
	CHECK(watch(kq, fds[0], EVFILT_READ, EV_ADD | EV_DISABLE, &first) == 0);
	CHECK(look(kq, events, 4) == 0);
	CHECK(watch(kq, fds[0], EVFILT_READ, EV_ENABLE, NULL) == 0);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[0], EVFILT_READ, 0));
	CHECK(watch(kq, fds[0], EVFILT_READ, EV_DISABLE, NULL) == 0);
	CHECK(look(kq, events, 4) == 0);
	// Added again, it is enabled unless EV_DISABLE comes with it, and has
	// the new udata.
	CHECK(watch(kq, fds[0], EVFILT_READ, EV_ADD | EV_DISABLE, &second) == 0);
	CHECK(look(kq, events, 4) == 0);
	CHECK(watch(kq, fds[0], EVFILT_READ, EV_ADD, &second) == 0);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[0], EVFILT_READ, 0) && events[0].udata == &second);
	close(fds[0]);
	close(fds[1]);
	close(kq);
}

// The number of the user event the steps below use.
#define USER_IDENT 0xee1eb9f4u

// EVFILT_USER: an event of the program's own, its flags and its data.
static void user(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int marker;
	struct kevent change, events[4] = {{0}};
	CHECK(watch(kq, USER_IDENT, EVFILT_USER, EV_ADD | EV_CLEAR, &marker) == 0);
	CHECK(look(kq, events, 4) == 0);
	// A trigger makes it ready, once.
	EV_SET(&change, USER_IDENT, EVFILT_USER, 0, NOTE_TRIGGER, 0, &marker);
	CHECK(kevent(kq, &change, 1, NULL, 0, NULL) == 0);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], USER_IDENT, EVFILT_USER, EV_CLEAR) && events[0].udata == &marker);
	CHECK(look(kq, events, 4) == 0);

	// The event's flags are copied, or'ed and and'ed by the changes, and
	// the last change's data comes back: 5, then 5|2, then 7&6.
	EV_SET(&change, USER_IDENT, EVFILT_USER, 0, NOTE_FFCOPY | 5, 0, &marker);
	CHECK(kevent(kq, &change, 1, events, 4, &NO_WAIT) == 0);
	EV_SET(&change, USER_IDENT, EVFILT_USER, 0, NOTE_FFOR | 2, 0, &marker);
	CHECK(kevent(kq, &change, 1, events, 4, &NO_WAIT) == 0);
	EV_SET(&change, USER_IDENT, EVFILT_USER, 0, NOTE_FFAND | 6 | NOTE_TRIGGER, 77, &marker);
	CHECK(kevent(kq, &change, 1, events, 4, &NO_WAIT) == 1);
	CHECK(events[0].ident == USER_IDENT && events[0].filter == EVFILT_USER);
	CHECK(events[0].fflags == 6 && events[0].data == 77 && events[0].flags == EV_CLEAR);
	// Reporting reset the flags and the data.
	CHECK(look(kq, events, 4) == 0);
	EV_SET(&change, USER_IDENT, EVFILT_USER, 0, NOTE_TRIGGER, 12, &marker);
	CHECK(kevent(kq, &change, 1, events, 4, &NO_WAIT) == 1);
	CHECK(events[0].fflags == 0 && events[0].data == 12);
	CHECK(watch(kq, USER_IDENT, EVFILT_USER, EV_DELETE, NULL) == 0);

	// Without EV_CLEAR an event that was triggered stays ready, with its
	// flags and data, until it is deleted. Here it is triggered as it is
	// added.
	EV_SET(&change, 7, EVFILT_USER, EV_ADD, NOTE_FFCOPY | 3 | NOTE_TRIGGER, 55, &marker);
	CHECK(kevent(kq, &change, 1, events, 4, &NO_WAIT) == 1);
	CHECK(events[0].ident == 7 && events[0].filter == EVFILT_USER && events[0].flags == 0);
	CHECK(events[0].fflags == 3 && events[0].data == 55);
	memset(events, 0, sizeof(events));
	CHECK(look(kq, events, 4) == 1);
	CHECK(events[0].ident == 7 && events[0].fflags == 3 && events[0].data == 55);
	CHECK(watch(kq, 7, EVFILT_USER, EV_DELETE, NULL) == 0);
	CHECK(look(kq, events, 4) == 0);
	change_fails(kq, 7, EVFILT_USER, EV_DELETE, ENOENT);
	close(kq);
}

// The name of the AF_UNIX listener below.
#define LISTENER "/tmp/events-listener"

// A connection comes in five seconds at most: accepts it, or returns -1.
static int accept_soon(int server) {
	if(!readable_soon(server))
		return -1;
	return accept(server, NULL, NULL);
}

// The two ends of a socket pair, watched for reading and writing, and what
// shutting and closing one does to the other.
static void socket_pair(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int fds[2];
	CHECK(socketpair(AF_UNIX, SOCK_STREAM, 0, fds) == 0);
	struct kevent events[4] = {{0}};
	CHECK(watch(kq, fds[0], EVFILT_READ, EV_ADD, NULL) == 0);
	CHECK(watch(kq, fds[1], EVFILT_READ, EV_ADD, NULL) == 0);
	CHECK(look(kq, events, 4) == 0);
	// A write on one end is something to read on the other.
	char byte;
	for(int from = 0; from < 2; from++) {
		CHECK(write(fds[from], "x", 1) == 1);
		CHECK(wait_for(kq, events, 4) == 1);
		CHECK(reports(&events[0], fds[1 - from], EVFILT_READ, 0));
		CHECK(read(fds[1 - from], &byte, 1) == 1);
		CHECK(look(kq, events, 4) == 0);
	}
	// Each end can be written at once.
	int writing = kqueue();
	CHECK(writing >= 0);
	CHECK(watch(writing, fds[0], EVFILT_WRITE, EV_ADD, NULL) == 0);
	CHECK(watch(writing, fds[1], EVFILT_WRITE, EV_ADD, NULL) == 0);
	CHECK(look(writing, events, 4) == 2);
	const struct kevent *first = find_event(events, 2, fds[0], EVFILT_WRITE);
	const struct kevent *second = find_event(events, 2, fds[1], EVFILT_WRITE);
	CHECK(first && reports(first, fds[0], EVFILT_WRITE, 0));
	CHECK(second && reports(second, fds[1], EVFILT_WRITE, 0));
	close(writing);

	// An end shut for writing is the end of the stream to the other.
	CHECK(shutdown(fds[0], SHUT_WR) == 0);
	int found = wait_for(kq, events, 4);
	CHECK(found >= 1 && find_event(events, found, fds[1], EVFILT_READ));
	// Shut both ways, an end has hung up, and the other still reads the end.
	CHECK(shutdown(fds[0], SHUT_RD) == 0);
	CHECK(wait_for(kq, events, 4) == 2);
	const struct kevent *hung_up = find_event(events, 2, fds[0], EVFILT_READ);
	CHECK(hung_up && (hung_up->flags & EV_EOF));
	CHECK(find_event(events, 2, fds[1], EVFILT_READ));
	// Closing an end ends its watch, and the other end's reports EV_EOF.
	close(fds[0]);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], fds[1], EVFILT_READ, EV_EOF));
	close(fds[1]);
	close(kq);
}

// A listening socket whose watch was added before it listened.
static void unix_listener(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int server = socket(AF_UNIX, SOCK_STREAM, 0);
	CHECK(server >= 0);
	struct kevent events[4] = {{0}};
	CHECK(watch(kq, server, EVFILT_READ, EV_ADD, NULL) == 0);
	struct sockaddr_un name;
	memset(&name, 0, sizeof(name));
	name.sun_family = AF_UNIX;
	strcpy(name.sun_path, LISTENER);
	socklen_t length = (socklen_t)(offsetof(struct sockaddr_un, sun_path) + strlen(LISTENER) + 1);
	unlink(LISTENER);
	CHECK(bind(server, (struct sockaddr *)&name, length) == 0);
	CHECK(listen(server, 4) == 0);
	CHECK(look(kq, events, 4) == 0);
	int client = socket(AF_UNIX, SOCK_STREAM, 0);
	CHECK(client >= 0);
	CHECK(connect(client, (struct sockaddr *)&name, length) == 0);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], server, EVFILT_READ, 0));
	int accepted = accept_soon(server);
	CHECK(accepted >= 0);
	CHECK(look(kq, events, 4) == 0);
	close(accepted);
	close(client);
	close(server);
	close(kq);
	unlink(LISTENER);
}

// How many TCP connections tcp_many makes, and how many of them are
// written to.
enum { CLIENTS = 200, WRITTEN = 10, GAP = CLIENTS / WRITTEN };

// Many connected TCP sockets, each watched for reading: one call finds
// exactly those that have something to read.
static void tcp_many(void) {
	int server = socket(AF_INET, SOCK_STREAM, 0);
	CHECK(server >= 0);
	struct sockaddr_in at;
	memset(&at, 0, sizeof(at));
	at.sin_family = AF_INET;
	at.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
	CHECK(bind(server, (struct sockaddr *)&at, sizeof(at)) == 0);
	CHECK(listen(server, 16) == 0);
	socklen_t length = sizeof(at);
	CHECK(getsockname(server, (struct sockaddr *)&at, &length) == 0);
	int clients[CLIENTS], accepted[CLIENTS];
	struct kevent changes[CLIENTS], events[64] = {{0}};
	for(int i = 0; i < CLIENTS; i++) {
		clients[i] = socket(AF_INET, SOCK_STREAM, 0);
		CHECK(clients[i] >= 0);
		CHECK(connect(clients[i], (struct sockaddr *)&at, sizeof(at)) == 0);
		accepted[i] = accept_soon(server);
		CHECK(accepted[i] >= 0);
		EV_SET(&changes[i], accepted[i], EVFILT_READ, EV_ADD, 0, 0, (void *)(uintptr_t)(i + 1));
	}
	int kq = kqueue();
	CHECK(kq >= 0);
	CHECK(kevent(kq, changes, CLIENTS, NULL, 0, NULL) == 0);
	CHECK(look(kq, events, 64) == 0);
	for(int i = 0; i < WRITTEN; i++)
		CHECK(write(clients[i * GAP], "x", 1) == 1);
	// The bytes take the loopback to arrive.
	for(int waited = 0; waited < 500; waited++) {
		int arrived = 0;
		for(int i = 0; i < WRITTEN; i++) {
			struct pollfd entry = {.fd = accepted[i * GAP], .events = POLLIN};
			arrived += poll(&entry, 1, 0) == 1;
		}
		if(arrived == WRITTEN)
			break;
		check_sleep_ms(10);
	}
	CHECK(wait_for(kq, events, 64) == WRITTEN);
	char seen[CLIENTS] = {0};
	for(int i = 0; i < WRITTEN; i++) {
		uintptr_t index = (uintptr_t)events[i].udata - 1;
		CHECK(index < CLIENTS && index % GAP == 0 && !seen[index]);
		if(index >= CLIENTS)
			continue;
		seen[index] = 1;
		CHECK(reports(&events[i], accepted[index], EVFILT_READ, 0));
	}
	for(int i = 0; i < CLIENTS; i++) {
		close(accepted[i]);
		close(clients[i]);
	}
	close(server);
	close(kq);
}

// Sockets: a pair, a listener, and two hundred TCP connections.
static void sockets(void) {
	socket_pair();
	unix_listener();
	tcp_many();
}

// What a thread that waits on a queue for one event found. A waiter is
// static where it is used, so that a thread that never returns cannot
// write into a frame that is gone.
struct waiter {
	int kq;
	int found;
	int error;
	struct kevent event;
	// Set last, by the thread itself.
	int finished;
};

// Waits on the queue of `argument`, a struct waiter, with no time limit.
static void *wait_forever(void *argument) {
	struct waiter *waiter = argument;
	errno = 0;
	waiter->found = kevent(waiter->kq, NULL, 0, &waiter->event, 1, NULL);
	waiter->error = errno;
	__atomic_store_n(&waiter->finished, 1, __ATOMIC_RELEASE);
	return NULL;
}

// Starts a thread that waits on `kq` for one event, with no time limit,
// and gives it time to block in kevent: it has not returned by then.
static pthread_t start_waiter(struct waiter *waiter, int kq) {
	memset(waiter, 0, sizeof(*waiter));
	waiter->kq = kq;
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, wait_forever, waiter) == 0);
	check_sleep_ms(50);
	CHECK(!__atomic_load_n(&waiter->finished, __ATOMIC_ACQUIRE));
	return thread;
}

// A trigger of the user event `ident`, which wakes the thread that waits
// on the queue.
static void woken_by_trigger(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	CHECK(watch(kq, 1, EVFILT_USER, EV_ADD | EV_CLEAR, NULL) == 0);
	static struct waiter waiter;
	pthread_t thread = start_waiter(&waiter, kq);
	struct kevent trigger;
	EV_SET(&trigger, 1, EVFILT_USER, 0, NOTE_TRIGGER, 0, NULL);
	CHECK(kevent(kq, &trigger, 1, NULL, 0, NULL) == 0);
	join_within(thread, &waiter.finished);
	CHECK(waiter.found == 1);
	CHECK(waiter.event.ident == 1 && waiter.event.filter == EVFILT_USER);
	close(kq);
}

// A write to a watched pipe wakes the thread that waits on the queue.
static void woken_by_write(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(watch(kq, fds[0], EVFILT_READ, EV_ADD, NULL) == 0);
	static struct waiter waiter;
	pthread_t thread = start_waiter(&waiter, kq);
	CHECK(write(fds[1], "w", 1) == 1);
	join_within(thread, &waiter.finished);
	CHECK(waiter.found == 1);
	CHECK(reports(&waiter.event, fds[0], EVFILT_READ, 0));
	close(fds[0]);
	close(fds[1]);
	close(kq);
}

// The handler of SIGALRM, which does nothing: it is there to be caught.
static void on_alarm(int number) {
	(void)number;
}

// A caught signal ends the wait with EINTR, though the handler does not
// ask for it, as it would for a read, to be restarted.
static void interrupted(void) {
	struct sigaction action = {0};
	action.sa_handler = on_alarm;
	sigemptyset(&action.sa_mask);
	CHECK(sigaction(SIGALRM, &action, NULL) == 0);
	int kq = kqueue();
	CHECK(kq >= 0);
	static struct waiter waiter;
	pthread_t thread = start_waiter(&waiter, kq);
	// A signal that came before the thread blocked is gone, so it is sent
	// again until the thread returns. It may have returned already when
	// the next is sent, which then finds no thread to signal.
	for(int tries = 0; tries < 100 && !__atomic_load_n(&waiter.finished, __ATOMIC_ACQUIRE);
			tries++) {
		pthread_kill(thread, SIGALRM);
		check_sleep_ms(50);
	}
	join_within(thread, &waiter.finished);
	CHECK(waiter.found == -1 && waiter.error == EINTR);
	signal(SIGALRM, SIG_DFL);
	close(kq);
}

// How many threads wait on one queue, and how many triggers and writes
// they are woken by.
enum { WORKERS = 4, ROUNDS = 1000 };

// The user event that ends the workers.
#define QUIT_IDENT 2

// What the workers of one queue share.
struct shared_queue {
	int kq;
	int pipe_read;
	// How many times the trigger event was reported, the most recent data
	// of it any worker saw, and how many bytes the workers read.
	int reports;
	int last_data;
	int bytes;
};

struct worker {
	struct shared_queue *queue;
	// Whether a wait ended with no event or with an error.
	int failed;
	// Set last, by the thread itself.
	int finished;
};

// Reads what the pipe holds, until it holds nothing.
static void drain(struct shared_queue *queue) {
	char buffer[64];
	for(;;) {
		ssize_t got = read(queue->pipe_read, buffer, sizeof(buffer));
		if(got <= 0)
			return;
		__atomic_add_fetch(&queue->bytes, (int)got, __ATOMIC_RELAXED);
	}
}

// Waits on the queue for events and handles them, until the event that
// ends the workers comes.
static void *work(void *argument) {
	struct worker *worker = argument;
	struct shared_queue *queue = worker->queue;
	struct kevent events[8];
	for(int quit = 0; !quit;) {
		int count = kevent(queue->kq, NULL, 0, events, 8, &PATIENCE);
		if(count <= 0) {
			worker->failed = 1;
			break;
		}
		for(int i = 0; i < count; i++) {
			if(events[i].filter == EVFILT_READ) {
				drain(queue);
			} else if(events[i].ident == QUIT_IDENT) {
				quit = 1;
			} else {
				__atomic_add_fetch(&queue->reports, 1, __ATOMIC_RELAXED);
				int value = (int)events[i].data;
				if(value > __atomic_load_n(&queue->last_data, __ATOMIC_RELAXED))
					__atomic_store_n(&queue->last_data, value, __ATOMIC_RELAXED);
			}
		}
	}
	__atomic_store_n(&worker->finished, 1, __ATOMIC_RELEASE);
	return NULL;
}

// Four threads wait on one queue while a user event is triggered a
// thousand times and a thousand bytes are written to a pipe, both
// edge-triggered. Two triggers may be reported as one, so the reports are
// not counted exactly: what must hold is that every byte is read, the last
// trigger is reported, and every thread returns.
static void many_wakes(void) {
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(fcntl(fds[0], F_SETFL, O_NONBLOCK) == 0);
	static struct shared_queue shared;
	shared.kq = kqueue();
	shared.pipe_read = fds[0];
	CHECK(shared.kq >= 0);
	CHECK(watch(shared.kq, 1, EVFILT_USER, EV_ADD | EV_CLEAR, NULL) == 0);
	CHECK(watch(shared.kq, fds[0], EVFILT_READ, EV_ADD | EV_CLEAR, NULL) == 0);
	static struct worker workers[WORKERS];
	pthread_t threads[WORKERS];
	for(int i = 0; i < WORKERS; i++) {
		memset(&workers[i], 0, sizeof(workers[i]));
		workers[i].queue = &shared;
		int created = pthread_create(&threads[i], NULL, work, &workers[i]) == 0;
		CHECK(created);
		// A thread that was not made has nothing to wait for.
		if(!created)
			workers[i].finished = 1;
	}
	for(int i = 1; i <= ROUNDS; i++) {
		struct kevent trigger;
		EV_SET(&trigger, 1, EVFILT_USER, 0, NOTE_TRIGGER, i, NULL);
		CHECK(kevent(shared.kq, &trigger, 1, NULL, 0, NULL) == 0);
		CHECK(write(fds[1], "x", 1) == 1);
		// Spread over time, so that the threads go to sleep and wake again.
		if(i % 10 == 0)
			check_sleep_ms(1);
	}
	for(int waited = 0; waited < 500; waited++) {
		if(__atomic_load_n(&shared.bytes, __ATOMIC_RELAXED) == ROUNDS
				&& __atomic_load_n(&shared.last_data, __ATOMIC_RELAXED) == ROUNDS)
			break;
		check_sleep_ms(10);
	}
	CHECK(__atomic_load_n(&shared.bytes, __ATOMIC_RELAXED) == ROUNDS);
	CHECK(__atomic_load_n(&shared.last_data, __ATOMIC_RELAXED) == ROUNDS);
	int reports = __atomic_load_n(&shared.reports, __ATOMIC_RELAXED);
	CHECK(reports >= 1 && reports <= ROUNDS);
	// The event that ends the workers stays ready, but only one thread
	// may be woken for it: it is triggered until every thread has seen it.
	CHECK(watch(shared.kq, QUIT_IDENT, EVFILT_USER, EV_ADD, NULL) == 0);
	for(int waited = 0; waited < 500; waited++) {
		int ended = 0;
		for(int i = 0; i < WORKERS; i++)
			ended += __atomic_load_n(&workers[i].finished, __ATOMIC_ACQUIRE);
		if(ended == WORKERS)
			break;
		struct kevent trigger;
		EV_SET(&trigger, QUIT_IDENT, EVFILT_USER, 0, NOTE_TRIGGER, 0, NULL);
		CHECK(kevent(shared.kq, &trigger, 1, NULL, 0, NULL) == 0);
		check_sleep_ms(10);
	}
	for(int i = 0; i < WORKERS; i++) {
		join_within(threads[i], &workers[i].finished);
		CHECK(!workers[i].failed);
	}
	// What was triggered and written was reported once: nothing is left.
	CHECK(watch(shared.kq, QUIT_IDENT, EVFILT_USER, EV_DELETE, NULL) == 0);
	struct kevent events[4] = {{0}};
	CHECK(look(shared.kq, events, 4) == 0);
	close(shared.kq);
	close(fds[0]);
	close(fds[1]);
}

// Threads that wait on a queue, and what wakes them.
static void wakes(void) {
	woken_by_trigger();
	woken_by_write();
	interrupted();
	many_wakes();
}

// How long a wait lasts: not less than its time, and not at all with a
// watch ready.
static void timeouts(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(watch(kq, fds[0], EVFILT_READ, EV_ADD, NULL) == 0);
	struct kevent events[4] = {{0}};
	// Nothing is ready: the call returns 0 once the time has passed.
	struct timespec twenty_ms = {0, 20000000};
	long long start = check_now_ns();
	CHECK(kevent(kq, NULL, 0, events, 4, &twenty_ms) == 0);
	long long took = check_now_ns() - start;
	CHECK(took >= 20000000 && took < 5000000000LL);
	// With no room for events the call makes its changes and does not wait.
	start = check_now_ns();
	CHECK(kevent(kq, NULL, 0, NULL, 0, &PATIENCE) == 0);
	CHECK(check_now_ns() - start < 1000000000LL);
	// A ready watch ends a long wait at once, and a time too long to count
	// is no limit rather than an error.
	CHECK(write(fds[1], "x", 1) == 1);
	start = check_now_ns();
	CHECK(kevent(kq, NULL, 0, events, 4, &PATIENCE) == 1);
	CHECK(check_now_ns() - start < 1000000000LL);
	struct timespec huge = {.tv_sec = INT64_MAX, .tv_nsec = 999999999};
	CHECK(kevent(kq, NULL, 0, events, 4, &huge) == 1);
	close(fds[0]);
	close(fds[1]);
	close(kq);
}

// A closed pipe is watched no more, whichever end went last.
static void closed_pipes(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int first[2], second[2];
	CHECK(pipe(first) == 0 && pipe(second) == 0);
	CHECK(watch(kq, first[0], EVFILT_READ, EV_ADD, NULL) == 0);
	CHECK(watch(kq, second[0], EVFILT_READ, EV_ADD, NULL) == 0);
	// The writer going first leaves the reader's end to report, which it
	// does not once it has gone too.
	close(first[0]);
	close(first[1]);
	close(second[1]);
	close(second[0]);
	struct kevent events[4] = {{0}};
	CHECK(look(kq, events, 4) == 0);
	change_fails(kq, first[0], EVFILT_READ, EV_DELETE, ENOENT);
	change_fails(kq, second[0], EVFILT_READ, EV_DELETE, ENOENT);
	close(kq);
}

// A watch is of a description, not of a number: closing one of several
// descriptors of it keeps the watch, and adding the number again for
// another description replaces it.
static void reused_numbers(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int first, second, third;
	struct kevent events[4] = {{0}};

	// Closed with nothing else holding the pipe, its number is a new
	// pipe's: nothing is watched until the number is added again.
	int old[2], fresh[2];
	CHECK(pipe(old) == 0 && pipe(fresh) == 0);
	int number = old[0];
	CHECK(watch(kq, number, EVFILT_READ, EV_ADD, &first) == 0);
	close(old[0]);
	close(old[1]);
	CHECK(dup2(fresh[0], number) == number);
	CHECK(write(fresh[1], "n", 1) == 1);
	CHECK(look(kq, events, 4) == 0);
	CHECK(watch(kq, number, EVFILT_READ, EV_ADD, &second) == 0);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], number, EVFILT_READ, 0) && events[0].udata == &second);
	close(number);
	close(fresh[0]);
	close(fresh[1]);

	// Another descriptor holds the pipe: closing the watched number keeps
	// the watch, which still reports the number, until another description
	// is added there.
	int kept[2], other[2];
	CHECK(pipe(kept) == 0 && pipe(other) == 0);
	number = kept[0];
	int copy = dup(number);
	CHECK(copy >= 0);
	CHECK(watch(kq, number, EVFILT_READ, EV_ADD, &third) == 0);
	close(number);
	CHECK(write(kept[1], "k", 1) == 1);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], number, EVFILT_READ, 0) && events[0].udata == &third);
	CHECK(dup2(other[0], number) == number);
	CHECK(watch(kq, number, EVFILT_READ, EV_ADD, &second) == 0);
	// The pipe the watch was on is still readable, but no longer watched.
	CHECK(look(kq, events, 4) == 0);
	CHECK(write(other[1], "o", 1) == 1);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], number, EVFILT_READ, 0) && events[0].udata == &second);
	close(number);
	close(copy);
	close(kept[1]);
	close(other[0]);
	close(other[1]);
	close(kq);
}

// A child uses the queue its parent made.
static void shared_by_fork(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	CHECK(watch(kq, 3, EVFILT_USER, EV_ADD | EV_CLEAR, NULL) == 0);
	pid_t pid = check_fork();
	if(pid == 0) {
		struct kevent trigger;
		EV_SET(&trigger, 3, EVFILT_USER, 0, NOTE_TRIGGER, 0, NULL);
		CHECK(kevent(kq, &trigger, 1, NULL, 0, NULL) == 0);
		check_exit_child();
	}
	struct kevent events[4] = {{0}};
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(events[0].ident == 3 && events[0].filter == EVFILT_USER);
	wait_for_child(pid);
	close(kq);
}

// A description the parent has closed but the child holds is still
// watched, as it is on Linux and unlike the BSDs, until the watch is
// deleted.
static void kept_by_child(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	int held[2], go[2];
	CHECK(pipe(held) == 0 && pipe(go) == 0);
	CHECK(watch(kq, held[0], EVFILT_READ, EV_ADD, NULL) == 0);
	pid_t pid = check_fork();
	if(pid == 0) {
		// A child that is stuck ends on its own.
		alarm(30);
		close(go[1]);
		char byte;
		// It writes when told to, and goes on holding the pipe until the
		// parent closes `go`.
		CHECK(readable_soon(go[0]) && read(go[0], &byte, 1) == 1);
		CHECK(write(held[1], "k", 1) == 1);
		CHECK(readable_soon(go[0]) && read(go[0], &byte, 1) == 0);
		check_exit_child();
	}
	close(go[0]);
	close(held[0]);
	close(held[1]);
	struct kevent events[4] = {{0}};
	CHECK(look(kq, events, 4) == 0);
	CHECK(write(go[1], "g", 1) == 1);
	CHECK(wait_for(kq, events, 4) == 1);
	CHECK(reports(&events[0], held[0], EVFILT_READ, 0));
	// The number is closed here, and the watch can still be deleted by it.
	CHECK(watch(kq, held[0], EVFILT_READ, EV_DELETE, NULL) == 0);
	CHECK(look(kq, events, 4) == 0);
	close(go[1]);
	wait_for_child(pid);
	change_fails(kq, held[0], EVFILT_READ, EV_DELETE, ENOENT);
	close(kq);
}

// How long a watch lasts: until the description it was added for goes, in
// every process that holds it, or it is deleted or replaced.
static void lifetime(void) {
	closed_pipes();
	reused_numbers();
	shared_by_fork();
	kept_by_child();
}

// poll on a queue: readable while a watch is queued to be looked at.
static void poll_queue(void) {
	int kq = kqueue();
	CHECK(kq >= 0);
	struct pollfd entry = {.fd = kq, .events = POLLIN};
	struct kevent events[4] = {{0}};
	// A queue that has nothing to look at is not readable; one with a
	// watch to look at is, though the look may find the watch not ready.
	CHECK(poll(&entry, 1, 0) == 0);
	CHECK(watch(kq, 1, EVFILT_USER, EV_ADD | EV_CLEAR, NULL) == 0);
	CHECK(look(kq, events, 4) == 0);
	CHECK(poll(&entry, 1, 0) == 0);
	struct kevent trigger;
	EV_SET(&trigger, 1, EVFILT_USER, 0, NOTE_TRIGGER, 0, NULL);
	CHECK(kevent(kq, &trigger, 1, NULL, 0, NULL) == 0);
	CHECK(poll(&entry, 1, 0) == 1 && entry.revents == POLLIN);
	CHECK(wait_for(kq, events, 4) == 1);

	// A write to a watched pipe makes the queue readable.
	int fds[2];
	CHECK(pipe(fds) == 0);
	CHECK(watch(kq, fds[0], EVFILT_READ, EV_ADD | EV_CLEAR, NULL) == 0);
	CHECK(look(kq, events, 4) == 0);
	CHECK(poll(&entry, 1, 0) == 0);
	CHECK(write(fds[1], "x", 1) == 1);
	CHECK(poll(&entry, 1, 5000) == 1 && entry.revents == POLLIN);
	CHECK(wait_for(kq, events, 4) == 1);
	close(fds[0]);
	close(fds[1]);
	close(kq);
}

// How many pipes the last step watches, and which of them is written to.
enum { PIPES = 1000, EVERY = 7 };

// A thousand pipes watched at once, a seventh of them written to.
static void many(void) {
	raise_files_limit();
	static int ends[PIPES][2];
	static struct kevent changes[PIPES];
	int kq = kqueue();
	CHECK(kq >= 0);
	for(int i = 0; i < PIPES; i++) {
		CHECK(pipe(ends[i]) == 0);
		EV_SET(&changes[i], ends[i][0], EVFILT_READ, EV_ADD | EV_CLEAR, 0, 0,
				(void *)(uintptr_t)(i + 1));
	}
	CHECK(kevent(kq, changes, PIPES, NULL, 0, NULL) == 0);
	struct kevent events[64] = {{0}};
	CHECK(look(kq, events, 64) == 0);
	// Every seventh pipe is written to: 143 of them.
	for(int i = 0; i < PIPES; i += EVERY)
		CHECK(write(ends[i][1], "x", 1) == 1);
	// Each call reports the next of them, in the order they were written.
	static char seen[PIPES];
	int total = 0, last = -1;
	for(int count = wait_for(kq, events, 64); count > 0; count = look(kq, events, 64)) {
		for(int i = 0; i < count; i++) {
			int index = (int)(uintptr_t)events[i].udata - 1;
			CHECK(index >= 0 && index < PIPES && index % EVERY == 0 && !seen[index]);
			if(index < 0 || index >= PIPES)
				continue;
			CHECK(reports(&events[i], ends[index][0], EVFILT_READ, EV_CLEAR));
			CHECK(index > last);
			seen[index] = 1;
			last = index;
			total++;
		}
	}
	CHECK(total == (PIPES + EVERY - 1) / EVERY);
	for(int i = 0; i < PIPES; i++) {
		close(ends[i][0]);
		close(ends[i][1]);
	}
	close(kq);
}

int main(void) {
	check_start("events");
	RUN(basics);
	RUN(errors);
	RUN(receipt);
	RUN(pipes);
	RUN(edge);
	RUN(edge_reads);
	RUN(oneshot_dispatch);
	RUN(disable);
	RUN(user);
	RUN(sockets);
	RUN(wakes);
	RUN(timeouts);
	RUN(lifetime);
	RUN(poll_queue);
	RUN(many);
	return check_end();
}
