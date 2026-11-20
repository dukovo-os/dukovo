/* SPDX-FileCopyrightText: 2026 The Dukovo contributors */
/* SPDX-License-Identifier: Apache-2.0 OR MIT */

/* kqueue(2) and kevent(2) as the BSDs declare them (ADR 0353): an event
 * queue that watches many descriptors at once, and hands out the ones
 * that are ready, however many others wait idle. It is for programs that
 * serve many connections, such as Go's runtime.
 *
 * A watch is named by its ident and filter: the descriptor and the way
 * it is watched, or for EVFILT_USER a number of the program's own. A
 * queue holds at most one watch for each pair. It is level-triggered
 * unless EV_CLEAR is set.
 *
 * Radix differs from the BSDs here:
 * - The filters are EVFILT_READ, EVFILT_WRITE and EVFILT_USER; any other
 *   fails with EINVAL. Timers, signals, processes and files are not
 *   watched.
 * - The data and fflags of a read or write event are 0, where the BSDs
 *   give the bytes to read or the room to write.
 * - A watch belongs to the description its descriptor named when it was
 *   added, and ends when that description goes: when the last of its
 *   descriptors, in every process, is closed. Closing one of several
 *   descriptors of it does not end the watch, as with Linux's epoll. It
 *   also ends when it is deleted, when its ident is added again for a
 *   descriptor that holds another description, and when the queue is
 *   closed.
 * - EV_DELETE removes a watch whether or not its descriptor is still
 *   open, where the BSDs fail with EBADF for a closed one.
 * - Files and directories, which are always ready, cannot be watched:
 *   EV_ADD fails with EPERM, as epoll_ctl does. Nor can another queue
 *   (EINVAL).
 * - fork shares the queue with the child, as Linux shares an epoll
 *   descriptor; the BSDs do not inherit one. kqueue() sets no
 *   close-on-exec: set it with fcntl. */

#ifndef _SYS_EVENT_H
#define _SYS_EVENT_H

#include <stdint.h>
#include <time.h>

#ifdef __cplusplus
extern "C" {
#endif

#define EVFILT_READ (-1)
#define EVFILT_WRITE (-2)
#define EVFILT_USER (-11)

/* What a change does (flags) and what an event says. EV_ERROR and EV_EOF
 * are for events; EV_SYSFLAGS and EV_FLAG1 are cleared from a change's
 * flags. */
#define EV_ADD 0x0001
#define EV_DELETE 0x0002
#define EV_ENABLE 0x0004
#define EV_DISABLE 0x0008
#define EV_ONESHOT 0x0010
#define EV_CLEAR 0x0020
#define EV_RECEIPT 0x0040
#define EV_DISPATCH 0x0080
#define EV_SYSFLAGS 0xf000
#define EV_FLAG1 0x2000
#define EV_ERROR 0x4000
#define EV_EOF 0x8000

/* The fflags of an EVFILT_USER change: NOTE_TRIGGER makes the event
 * ready; the low 24 bits (NOTE_FFLAGSMASK) are the event's own flags, and
 * the two bits of NOTE_FFCTRLMASK say how the change's apply to them:
 * NOTE_FFNOP leaves them, NOTE_FFAND keeps those in both, NOTE_FFOR adds
 * the change's, and NOTE_FFCOPY replaces them. */
#define NOTE_TRIGGER 0x01000000u
#define NOTE_FFNOP 0x00000000u
#define NOTE_FFAND 0x40000000u
#define NOTE_FFOR 0x80000000u
#define NOTE_FFCOPY 0xc0000000u
#define NOTE_FFCTRLMASK 0xc0000000u
#define NOTE_FFLAGSMASK 0x00ffffffu

/* A change for kevent to make, or an event it reports (32 bytes, the
 * kernel's own layout). A watch's events carry the udata it was last
 * added with. */
struct kevent {
	uintptr_t ident;
	short filter;
	unsigned short flags;
	unsigned int fflags;
	int64_t data;
	void *udata;
};

/* Sets the six fields of the struct kevent that kevp points to. */
#define EV_SET(kevp, a, b, c, d, e, f) \
	do { \
		struct kevent *event_set_ = (kevp); \
		event_set_->ident = (a); \
		event_set_->filter = (b); \
		event_set_->flags = (c); \
		event_set_->fflags = (d); \
		event_set_->data = (e); \
		event_set_->udata = (f); \
	} while(0)

/* Makes an event queue in the lowest free descriptor and returns it, or
 * returns -1 with errno EMFILE or ENOMEM. */
int kqueue(void);

/* Makes the nchanges changes at changelist, in order, then waits until a
 * watch of the queue kq is ready, for at most *timeout (not at all if it
 * is {0, 0}; as long as it takes if it is null), and writes up to nevents
 * events at eventlist. Returns how many it wrote. It waits only if
 * nevents is more than 0, and is then a cancellation point. Each change
 * is one of:
 * - EV_ADD: adds the watch, or, if the queue has it, sets its udata (and
 *   an EVFILT_USER event's fflags and data), leaving the EV_ONESHOT,
 *   EV_CLEAR and EV_DISPATCH it was added with. Either way the watch is
 *   enabled, unless EV_DISABLE is set without EV_ENABLE.
 * - EV_DELETE: removes the watch.
 * - Neither: sets the udata of the watch the queue has, and applies
 *   EV_ENABLE and EV_DISABLE, and for EVFILT_USER the fflags, with
 *   NOTE_TRIGGER.
 * EV_ONESHOT deletes the watch once it is reported, EV_DISPATCH disables
 * it, and EV_CLEAR reports it only when what it watches changes. A change
 * of filter 0 is passed over. EV_ADD together with EV_DELETE is EINVAL.
 *
 * A change that fails, and one with EV_RECEIPT, is reported as an event
 * with EV_ERROR whose data is the errno (0 for a receipt of a change that
 * worked), while eventlist has room; if any event was written that way
 * kevent returns them alone and reports no watch. Without room, a change
 * that fails fails kevent with its errno. The errors are EINVAL (an
 * unknown filter, or an event queue), EBADF (a descriptor that is not
 * open), EPERM (a file or directory), ENOENT (a watch the queue does not
 * hold, other than for EV_ADD) and ENOMEM.
 *
 * kevent fails with EBADF if kq is not an event queue, EINTR if a caught
 * signal comes while it waits, EFAULT for an address it cannot use, and
 * EINVAL for a negative nchanges or nevents, more than 8192 changes, or
 * a timeout that is not a time. */
int kevent(int kq, const struct kevent *changelist, int nchanges, struct kevent *eventlist,
		int nevents, const struct timespec *timeout);

#ifdef __cplusplus
}
#endif

#endif /* _SYS_EVENT_H */
