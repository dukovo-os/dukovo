// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Terminals on Radix (ADR 0066): termios, window sizes, job control and
// pseudo-terminals, all through a terminal's `control` method, which
// takes Linux's ioctl requests (radix-terminal(2)).

#include <asm/ioctls.h>
#include <bits/ensure.h>
#include <errno.h>
#include <fcntl.h>
#include <mlibc/all-sysdeps.hpp>
#include <mlibc/threads.hpp>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>

#include "radix.hpp"

namespace {

static_assert(sizeof(struct termios) == sizeof(struct radix_termios),
		"struct termios is the kernel's");
static_assert(sizeof(struct winsize) == sizeof(struct radix_window_size),
		"struct winsize is the kernel's");

// Sends `request` to the terminal in `fd`, with `argument` and the
// `size` bytes at `buffer`. Returns 0 or an errno.
int control(int fd, uint32_t request, uint64_t argument, const void *buffer, size_t size,
		uint64_t *result = nullptr) {
	uint32_t slot = radix::slot_of(fd);
	if(slot == radix::NO_SLOT)
		return EBADF;
	struct radix_file_control_results r;
	uint64_t status = radix_file_control(slot, request, argument,
			reinterpret_cast<uint64_t>(buffer), size, &r);
	if(status) {
		// Directories and other capabilities that are not files are not
		// terminals either.
		if(RADIX_STATUS_DOMAIN(status) == RADIX_KERNEL_ERROR_DOMAIN
				&& (RADIX_STATUS_CODE(status) == RADIX_KERNEL_WRONG_KIND
						|| RADIX_STATUS_CODE(status) == RADIX_KERNEL_NO_SUCH_METHOD))
			return ENOTTY;
		return radix::to_errno(status);
	}
	if(result)
		*result = r.result;
	return 0;
}

// Linux's BTRFS_IOC_GET_SUBVOL_INFO, _IOR(0x94, 60, struct
// btrfs_ioctl_get_subvol_info_args): what Btrfs keeps of the subvolume a
// directory is in, which the directory's `subvolume` method gives (ADR
// 0461).
constexpr unsigned long BTRFS_IOC_GET_SUBVOL_INFO = 0x81f8943c;

// Linux's struct btrfs_ioctl_timespec.
struct btrfs_time {
	uint64_t sec;
	uint32_t nsec;
};

// Linux's struct btrfs_ioctl_get_subvol_info_args.
struct btrfs_subvol_info {
	uint64_t treeid;
	char name[256];
	uint64_t parent_id;
	uint64_t dirid;
	uint64_t generation;
	uint64_t flags;
	uint8_t uuid[16];
	uint8_t parent_uuid[16];
	uint8_t received_uuid[16];
	uint64_t ctransid;
	uint64_t otransid;
	uint64_t stransid;
	uint64_t rtransid;
	struct btrfs_time ctime;
	struct btrfs_time otime;
	struct btrfs_time stime;
	struct btrfs_time rtime;
	uint64_t reserved[8];
};

static_assert(sizeof(struct btrfs_subvol_info) == ((BTRFS_IOC_GET_SUBVOL_INFO >> 16) & 0x3fff),
		"struct btrfs_subvol_info is Linux's");

// Fills `out` with what the directory in `fd` says of its subvolume.
// Returns 0 or an errno: ENOTTY for a file that is not a directory, or a
// directory on another filesystem, as Linux's ioctl says on one.
int subvolume_info(int fd, struct btrfs_subvol_info *out) {
	uint32_t slot = radix::slot_of(fd);
	if(slot == radix::NO_SLOT)
		return EBADF;
	unsigned char buffer[sizeof(struct radix_subvolume_info) + 255];
	uint64_t status = radix_directory_subvolume(slot, ".", 1, buffer, sizeof(buffer));
	if(status) {
		if(RADIX_STATUS_DOMAIN(status) == RADIX_KERNEL_ERROR_DOMAIN
				&& (RADIX_STATUS_CODE(status) == RADIX_KERNEL_WRONG_KIND
						|| RADIX_STATUS_CODE(status) == RADIX_KERNEL_NO_SUCH_METHOD
						|| RADIX_STATUS_CODE(status) == RADIX_KERNEL_NOT_SUPPORTED))
			return ENOTTY;
		return radix::to_errno(status);
	}
	struct radix_subvolume_info info;
	memcpy(&info, buffer, sizeof(info));
	memset(out, 0, sizeof(*out));
	out->treeid = info.id;
	size_t len = info.name_length < 255 ? info.name_length : 255;
	memcpy(out->name, buffer + sizeof(info), len);
	out->parent_id = info.parent;
	out->dirid = info.directory;
	out->generation = info.generation;
	out->flags = info.flags;
	memcpy(out->uuid, info.uuid, 16);
	memcpy(out->parent_uuid, info.parent_uuid, 16);
	memcpy(out->received_uuid, info.received_uuid, 16);
	out->ctransid = info.changed_transaction;
	out->otransid = info.made_transaction;
	out->stransid = info.sent_transaction;
	out->rtransid = info.received_transaction;
	const uint64_t times[4] = {info.changed, info.made, info.sent, info.received};
	struct btrfs_time *to[4] = {&out->ctime, &out->otime, &out->stime, &out->rtime};
	for(int i = 0; i < 4; i++) {
		to[i]->sec = times[i] / 1000000000;
		to[i]->nsec = static_cast<uint32_t>(times[i] % 1000000000);
	}
	return 0;
}

// The size of what Linux's terminal request `request` reads or writes.
size_t size_of(unsigned long request) {
	switch(request) {
	case TCGETS:
	case TCSETS:
	case TCSETSW:
	case TCSETSF:
		return sizeof(struct termios);
	case TIOCGWINSZ:
	case TIOCSWINSZ:
		return sizeof(struct winsize);
	case TIOCGPGRP:
	case TIOCSPGRP:
	case TIOCGSID:
	case FIONREAD:
		return sizeof(int);
	default:
		// The size requests made with _IOC carry.
		return _IOC_SIZE(request);
	}
}

// The pseudo-terminal number of the terminal in `fd`.
int pty_number(int fd, uint32_t *number) {
	return control(fd, RADIX_TERMINAL_GET_PTY_NUMBER, 0, number, sizeof(*number));
}

// Writes "/devices/pts/N" for `number` to `buf`, which holds `size` bytes.
int pts_path(uint32_t number, char *buf, size_t size) {
	char digits[10];
	int n = 0;
	do {
		digits[n++] = static_cast<char>('0' + number % 10);
		number /= 10;
	} while(number);
	const char prefix[] = "/devices/pts/";
	size_t len = sizeof(prefix) - 1;
	if(size < len + n + 1)
		return ERANGE;
	memcpy(buf, prefix, len);
	while(n)
		buf[len++] = digits[--n];
	buf[len] = 0;
	return 0;
}

} // namespace

namespace mlibc {

int Sysdeps<Isatty>::operator()(int fd) {
	struct termios attr;
	return control(fd, RADIX_TERMINAL_GET_ATTRIBUTES, 0, &attr, sizeof(attr));
}

int Sysdeps<Tcgetattr>::operator()(int fd, struct termios *attr) {
	return control(fd, RADIX_TERMINAL_GET_ATTRIBUTES, 0, attr, sizeof(*attr));
}

int Sysdeps<Tcsetattr>::operator()(int fd, int optional_action, const struct termios *attr) {
	uint32_t request;
	switch(optional_action) {
	case TCSANOW:
		request = RADIX_TERMINAL_SET_ATTRIBUTES;
		break;
	case TCSADRAIN:
		request = RADIX_TERMINAL_SET_ATTRIBUTES_DRAIN;
		break;
	case TCSAFLUSH:
		request = RADIX_TERMINAL_SET_ATTRIBUTES_FLUSH;
		break;
	default:
		return EINVAL;
	}
	return control(fd, request, 0, attr, sizeof(*attr));
}

int Sysdeps<Tcflow>::operator()(int fd, int action) {
	// TCOOFF, TCOON, TCIOFF and TCION are the kernel's.
	return control(fd, RADIX_TERMINAL_FLOW, static_cast<uint64_t>(action), nullptr, 0);
}

int Sysdeps<Tcflush>::operator()(int fd, int queue) {
	// TCIFLUSH, TCOFLUSH and TCIOFLUSH are the kernel's.
	return control(fd, RADIX_TERMINAL_FLUSH, static_cast<uint64_t>(queue), nullptr, 0);
}

int Sysdeps<Tcdrain>::operator()(int fd) {
	// A terminal's output is in its master's ring as soon as it is
	// written: there is nothing to wait for, but it is a cancellation
	// point all the same.
	thread_testcancel();
	return sysdep<Isatty>(fd);
}

int Sysdeps<Tcsendbreak>::operator()(int fd, int dur) {
	(void)dur;
	// Pseudo-terminals have no line to send a break on.
	return sysdep<Isatty>(fd);
}

int Sysdeps<Tcgetwinsize>::operator()(int fd, struct winsize *winsz) {
	return control(fd, RADIX_TERMINAL_GET_WINDOW_SIZE, 0, winsz, sizeof(*winsz));
}

int Sysdeps<Tcsetwinsize>::operator()(int fd, const struct winsize *winsz) {
	return control(fd, RADIX_TERMINAL_SET_WINDOW_SIZE, 0, winsz, sizeof(*winsz));
}

int Sysdeps<Ioctl>::operator()(int fd, unsigned long request, void *arg, int *result) {
	switch(request) {
	case FIONBIO: {
		// Non-blocking or not, as fcntl's F_SETFL.
		uint32_t slot = radix::slot_of(fd);
		if(slot == radix::NO_SLOT)
			return EBADF;
		struct radix_file_status_flags_results r;
		if(uint64_t status = radix_file_status_flags(slot, false, 0, &r); status)
			return radix::to_errno(status);
		uint32_t flags = r.flags & ~RADIX_DESCRIPTION_FLAGS_NON_BLOCKING;
		if(*static_cast<int *>(arg))
			flags |= RADIX_DESCRIPTION_FLAGS_NON_BLOCKING;
		if(uint64_t status = radix_file_status_flags(slot, true, flags, &r); status)
			return radix::to_errno(status);
		*result = 0;
		return 0;
	}
	case FIOCLEX:
	case FIONCLEX: {
		uint32_t slot = radix::slot_of(fd);
		if(slot == radix::NO_SLOT)
			return EBADF;
		uint32_t flags = request == FIOCLEX ? RADIX_SLOT_FLAGS_CLOSE_ON_EXEC : 0;
		struct radix_capability_flags_results r;
		if(uint64_t status = radix_capability_flags(slot, true, flags, &r); status)
			return radix::to_errno(status);
		*result = 0;
		return 0;
	}
	case TCSBRK:
		return sysdep<Isatty>(fd);
	case RADIX_FILE_CLONE: {
		// Linux's FICLONE, whose argument is the source's descriptor: the
		// kernel takes its slot (ADR 0139).
		uint32_t source = radix::slot_of(static_cast<int>(reinterpret_cast<intptr_t>(arg)));
		if(source == radix::NO_SLOT)
			return EBADF;
		if(int e = control(fd, RADIX_FILE_CLONE, source, nullptr, 0); e)
			return e;
		*result = 0;
		return 0;
	}
	case BTRFS_IOC_GET_SUBVOL_INFO: {
		if(int e = subvolume_info(fd, static_cast<struct btrfs_subvol_info *>(arg)); e)
			return e;
		*result = 0;
		return 0;
	}
	case RADIX_FILE_CLONE_RANGE: {
		// FICLONERANGE's struct file_clone_range, with the source's slot
		// for its src_fd.
		uint64_t range[4];
		memcpy(range, arg, sizeof(range));
		uint32_t source = radix::slot_of(static_cast<int>(range[0]));
		if(source == radix::NO_SLOT)
			return EBADF;
		range[0] = source;
		if(int e = control(fd, RADIX_FILE_CLONE_RANGE, 0, range, sizeof(range)); e)
			return e;
		*result = 0;
		return 0;
	}
	default:
		break;
	}
	if(request > UINT32_MAX)
		return EINVAL;
	// The kernel takes Linux's requests as they are: the argument as a
	// number and, for requests that pass a structure, as its address.
	uint64_t value;
	if(int e = control(fd, static_cast<uint32_t>(request), reinterpret_cast<uint64_t>(arg), arg,
			   size_of(request), &value);
			e)
		return e;
	*result = static_cast<int>(value);
	return 0;
}

int Sysdeps<Ttyname>::operator()(int fd, char *buf, size_t size) {
	// A pseudo-terminal is /devices/pts/N.
	uint32_t number;
	if(pty_number(fd, &number) == 0)
		return pts_path(number, buf, size);
	// A serial line, such as the first serial port, says its own name in
	// /devices (ADR 0318).
	char name[RADIX_TERMINAL_LINE_NAME_SIZE + 1] = {};
	if(int e = control(fd, RADIX_TERMINAL_GET_LINE_NAME, 0, name, RADIX_TERMINAL_LINE_NAME_SIZE); e)
		return e;
	const char prefix[] = "/devices/";
	size_t len = sizeof(prefix) - 1;
	size_t name_len = strlen(name);
	if(size < len + name_len + 1)
		return ERANGE;
	memcpy(buf, prefix, len);
	memcpy(buf + len, name, name_len + 1);
	return 0;
}

int Sysdeps<Ptsname>::operator()(int fd, char *buffer, size_t length) {
	uint32_t number;
	if(int e = pty_number(fd, &number); e)
		return e;
	return pts_path(number, buffer, length);
}

int Sysdeps<Unlockpt>::operator()(int fd) {
	uint32_t unlock = 0;
	return control(fd, RADIX_TERMINAL_SET_PTY_LOCK, 0, &unlock, sizeof(unlock));
}

} // namespace mlibc
