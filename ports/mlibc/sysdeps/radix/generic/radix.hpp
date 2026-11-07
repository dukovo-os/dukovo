// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Radix system layer's files share (radix-posix(7), ADR 0068):
// the slots a program starts with, the mapping from a kernel status to an
// errno, file descriptors as capability slots, and paths, which the C
// library resolves against its working directory to a path beneath the
// root directory, the only form the kernel takes.

#pragma once

#include <stddef.h>
#include <stdint.h>

#include <radix/radix.h>

struct iovec;
struct stat;

namespace radix {

// The slots every program starts with (radix-startup(2)).
constexpr uint32_t SLOT_THREAD = RADIX_STARTUP_SLOT_THREAD;
constexpr uint32_t SLOT_SPACE = RADIX_STARTUP_SLOT_SPACE;
constexpr uint32_t SLOT_RANDOM = RADIX_STARTUP_SLOT_RANDOM;
constexpr uint32_t SLOT_LOG = RADIX_STARTUP_SLOT_LOG;
constexpr uint32_t SLOT_PROCESS = RADIX_STARTUP_SLOT_PROCESS;
constexpr uint32_t SLOT_ROOT = RADIX_STARTUP_SLOT_ROOT;

// File descriptor `n` is slot FD_BASE + n.
constexpr uint32_t FD_BASE = RADIX_STARTUP_SLOT_FD_BASE;

// The most slots a capability space may have (radix-process(2)).
constexpr uint32_t MAX_SLOTS = RADIX_PROCESS_MAX_SLOTS;

// The most file descriptors a program can have.
constexpr int MAX_FDS = MAX_SLOTS - FD_BASE;

// No slot, for `start`.
constexpr uint32_t NO_SLOT = RADIX_ENDPOINT_NO_SLOT;

// The longest path the kernel takes, with its NUL.
constexpr size_t PATH_MAX_BYTES = RADIX_DIRECTORY_PATH_MAX;

// The errno a kernel status maps to: 0 for success.
int to_errno(uint64_t status);

// What a masked cancellation point returns for a cancel that is due: no
// status the kernel gives. to_errno maps it to ECANCELED.
constexpr uint64_t CANCELED = UINT64_MAX;

// The system call function (radix/abi.h's radix_syscall_fn) of a
// cancellation point, which acts on a cancel that is due when the call is
// to be made, or that came while it waited and a signal interrupted it
// before it did anything: the thread ends (pthread_cancel(3)). The
// x86_64/cancel.S stub makes the call; the C library's own calls, and the
// dynamic linker's, are plain ones. `cancellable_masked` returns CANCELED
// instead, for a caller that acts on the cancel itself once it has put
// back what it holds.
#ifdef MLIBC_BUILDING_RTLD
inline uint64_t cancellable(uint64_t number, const uint64_t in[10], uint64_t out[10]) {
	return radix_syscall(number, in, out);
}
#else
uint64_t cancellable(uint64_t number, const uint64_t in[10], uint64_t out[10]);
uint64_t cancellable_masked(uint64_t number, const uint64_t in[10], uint64_t out[10]);
#endif

// The slot of file descriptor `fd`, or NO_SLOT if it cannot be one.
inline uint32_t slot_of(int fd) {
	if(fd < 0 || fd >= MAX_FDS)
		return NO_SLOT;
	return FD_BASE + static_cast<uint32_t>(fd);
}

// One more than the highest file descriptor the program may have: the
// soft RLIMIT_NOFILE, at most MAX_FDS (radix-posix(7)).
int files_limit();

// Notes that setrlimit made `limit` the soft RLIMIT_NOFILE.
void set_files_limit(uint64_t limit);

// The C library keeps which file descriptors are taken (ADR 0403), so
// that an open finds the lowest free one without asking the kernel: a bit
// for each, set from the kernel's slots when the program starts and in a
// child after fork (read_fd_table), and by the C library as it opens and
// closes descriptors. A capability that reaches a slot some other way is
// found when a call to fill the slot answers `occupied`, which marks it
// taken; one emptied some other way stays taken until the program closes
// it. The C library's opens look past such a descriptor, which is never
// handed out twice.
//
// The dynamic linker opens files too, the libraries a program needs and
// dlopen's, with its own copy of this layer: a program that has one
// shares one table, the dynamic linker's (radix.cpp).
struct FdTable {
	// A bit for each descriptor, set while it is taken. Set and cleared
	// with atomic operations alone, as the notes of directory paths are
	// taken: open and close are async-signal-safe.
	uint64_t taken[(MAX_FDS + 63) / 64];
	// How many slots the capability space has, as last read or grown to.
	// Descriptors at or past it need the space grown (reach_slot).
	uint32_t slots;
	// Whether read_fd_table has filled it: not before the program starts,
	// when the dynamic linker opens the first library.
	bool read;
};

// A file descriptor's slot that free_slot chose and marked taken for the
// running thread. Unless the thread keeps it (keep: the slot was filled),
// it is free again once the claim goes out of scope.
//
// The kernel fills a slot only when the call that fills it has done its
// work: an open looks that the slot is empty, makes the file, and then
// puts it there. Two threads that chose the same lowest free descriptor
// would race for it, and the one that lost would choose again with its
// work done: an O_EXCL open would then fail with EEXIST on the file it
// had made itself.
class SlotClaim {
public:
	SlotClaim() = default;
	SlotClaim(const SlotClaim &) = delete;
	SlotClaim &operator=(const SlotClaim &) = delete;
	~SlotClaim();

	// The slot, or NO_SLOT until free_slot chose one.
	uint32_t slot() const {
		return slot_;
	}

	// The slot holds a capability now, the one the thread put there or
	// another (`occupied`): its descriptor stays taken.
	void keep() {
		kept_ = true;
	}

	// After a call that fills two slots at once answered `occupied`: keeps
	// the slot if it is full, which the kernel is asked.
	void keep_if_full();

private:
	friend int free_slot(int min_fd, SlotClaim *claim);
	uint32_t slot_ = NO_SLOT;
	bool kept_ = false;
};

// Finds the lowest free file descriptor from `min_fd`, marks it taken for
// the running thread, and grows the capability space if the slot is past
// its end. Returns 0 with the slot in `claim`, a new claim, or EMFILE if
// there is none below files_limit(). A capability may still reach the
// slot some other way first: callers that get `occupied` keep the claim
// and look again (`place`).
int free_slot(int min_fd, SlotClaim *claim);

// Grows the capability space, if need be, until it has slot `slot`.
// Returns 0, or EMFILE if it cannot.
int reach_slot(uint32_t slot);

// Marks descriptor `fd`, which a call just filled, taken.
void fd_taken(int fd);

// Marks descriptor `fd`, whose slot a call just emptied, free.
void fd_freed(int fd);

// Empties descriptor `fd`'s slot and marks it free: close, without the
// directory path it may have. Returns the kernel's status.
uint64_t delete_fd(int fd);

// Reads which descriptors are taken from the kernel's slots: when the
// program starts, and in a child after fork, where the threads that were
// opening and closing descriptors are not. Only while no other thread
// runs.
void read_fd_table();

// Forgets the path of directory descriptor `fd` (below).
void forget_dir_path(int fd);

// Runs `put(slot)`, which puts a capability in an empty slot, on the
// lowest free descriptor from `min_fd` until one is empty when it runs.
// Returns 0 and the descriptor, or an errno.
template<typename F>
int place(int min_fd, int *fd, F put) {
	for(;;) {
		SlotClaim claim;
		if(int e = free_slot(min_fd, &claim); e)
			return e;
		uint64_t status = put(claim.slot());
		if(!status) {
			claim.keep();
			*fd = static_cast<int>(claim.slot() - FD_BASE);
			// A new descriptor has no directory path until it is given one.
			forget_dir_path(*fd);
			return 0;
		}
		// Something else filled it first: it stays taken.
		if(RADIX_STATUS_DOMAIN(status) == RADIX_KERNEL_ERROR_DOMAIN
				&& RADIX_STATUS_CODE(status) == RADIX_KERNEL_OCCUPIED) {
			claim.keep();
			continue;
		}
		return to_errno(status);
	}
}

// A path as the kernel takes it: the directory slot it starts from and the
// path beneath it, never starting with `/` (`.` for the directory itself).
struct Path {
	uint32_t dir;
	size_t len;
	char text[PATH_MAX_BYTES];
};

// Resolves `path`, relative to the directory `dirfd` (AT_FDCWD for the
// working directory): an absolute path, or one relative to the working
// directory, becomes a path beneath the root, whose `.`, `..` and
// symbolic links the kernel works out. A path with a `..` in it,
// relative to a directory descriptor whose path is known, becomes one
// beneath the root too. Returns 0 or an errno.
int resolve(int dirfd, const char *path, Path *out);

// The paths of directory descriptors (ADR 0089). The kernel resolves a
// path beneath a directory capability and never above it, so the C
// library notes the path, beneath the root, by which each directory
// descriptor was opened, and goes by it from the root for a `..` beneath
// the descriptor and for fchdir. A descriptor from before exec, or one
// opened while the notes are full, has no path.

// Notes that descriptor `fd`, just opened as a directory by `path`, has
// the path `path` leads to from the root; one beneath a descriptor with
// no path has none.
void note_dir_path(int fd, const Path &path);

// Gives descriptor `to` the path of `from`, or none if it has none, as a
// copy of `from` it now is.
void copy_dir_path(int from, int to);

// Forgets the path of `fd`, which was closed or is to be another file.
void forget_dir_path(int fd);

// Copies the path of descriptor `fd`, beneath the root (`.` for the root
// itself), to `out`, which has room for PATH_MAX_BYTES bytes, and returns
// its length: 0 if it has none.
size_t dir_path(int fd, char *out);

// The working directory, absolute, as the program knows it.
const char *cwd();

// Makes `path` (absolute, with no links in it: what the kernel's
// real_path returned) the working directory.
int set_cwd(const char *path);

// Called at start-up with the auxiliary vector's entries.
void init_from_auxv(const uintptr_t *auxv);

// The first thread's stack: its lowest address and size. Returns 0, or
// ENOSYS if start-up did not find it.
int first_stack(void **base, size_t *size);

// The number of the calling process, cached until fork.
uint64_t pid();

// Forgets the cached process number, in a child after fork.
void forget_pid();

// A Linux-style `struct stat` from what the kernel says about a file.
void fill_stat(struct ::stat *st, const struct radix_file_status &f);

// The cursor of the directory listing descriptor `fd` reads next.
uint64_t dir_cursor(int fd);
void set_dir_cursor(int fd, uint64_t cursor);

// The kernel's ClockKind for Linux's clock number `clock`, or EINVAL for
// one Linux does not have.
int clock_kind(int clock, enum radix_clock_kind *kind);

// The bytes of the `count` buffers at `iov` in all, in `sum`; EINVAL if
// they pass SSIZE_MAX, as readv, writev, sendmsg and recvmsg fail then
// on Linux.
int iov_total(const struct iovec *iov, size_t count, size_t *sum);

// Writes `len` bytes of `text` to the kernel log.
void log(const char *text, size_t len);

// The number of the interface that has the IPv4 address `address` (in
// network byte order), as Linux's ip_dev_find: 0 if none has it
// (interfaces.cpp).
unsigned int interface_with(uint32_t address);

// The first IPv4 address of the interface numbered `index`, the one it
// sends from, in network byte order: 0 if it has none.
uint32_t interface_address(unsigned int index);

} // namespace radix
