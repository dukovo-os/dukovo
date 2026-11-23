// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// inotify(7), sys/inotify.h's (ADR 0615): a watch file is the capability
// radix-process(2)'s `watch_file` puts in the lowest free descriptor,
// inotify_add_watch is radix-file(2)'s `watch_add` of it and
// inotify_rm_watch its `watch_remove`. The masks and the records a read
// gives are Linux's own, which the kernel takes and gives as they are.

#include <abi-bits/fcntl.h>
#include <abi-bits/inotify.h>
#include <errno.h>
#include <mlibc/all-sysdeps.hpp>
#include <stdint.h>

#include "radix.hpp"

namespace {

// Whether `status` is the kernel error `code`.
bool is(uint64_t status, uint16_t code) {
	return RADIX_STATUS_DOMAIN(status) == RADIX_KERNEL_ERROR_DOMAIN
			&& RADIX_STATUS_CODE(status) == code;
}

// The errno of a failed watch method: a descriptor that is not a watch
// file is EINVAL, as Linux gives for one that is not an inotify
// descriptor, which includes a capability of another kind.
int watch_errno(uint64_t status) {
	if(is(status, RADIX_KERNEL_NO_SUCH_METHOD) || is(status, RADIX_KERNEL_WRONG_KIND))
		return EINVAL;
	return radix::to_errno(status);
}

} // namespace

namespace mlibc {

int Sysdeps<InotifyCreate>::operator()(int flags, int *fd) {
	if(flags & ~(IN_NONBLOCK | IN_CLOEXEC))
		return EINVAL;
	uint32_t watch_flags = 0;
	if(flags & IN_NONBLOCK)
		watch_flags |= RADIX_PIPE_FLAGS_NON_BLOCKING;
	if(flags & IN_CLOEXEC)
		watch_flags |= RADIX_PIPE_FLAGS_CLOSE_ON_EXEC;
	return radix::place(0, fd, [&](uint32_t slot) {
		return radix_process_watch_file(radix::SLOT_PROCESS, slot, watch_flags);
	});
}

int Sysdeps<InotifyAddWatch>::operator()(int ifd, const char *path, uint32_t mask, int *wd) {
	uint32_t slot = radix::slot_of(ifd);
	if(slot == radix::NO_SLOT)
		return EBADF;
	return radix::at(AT_FDCWD, path, [&](const radix::Path &resolved) {
		struct radix_file_watch_add_results r;
		uint64_t status = radix_file_watch_add(slot, resolved.dir, resolved.text, resolved.len,
				mask, &r);
		if(status)
			return watch_errno(status);
		*wd = static_cast<int>(r.watch);
		return 0;
	});
}

int Sysdeps<InotifyRmWatch>::operator()(int ifd, int wd) {
	uint32_t slot = radix::slot_of(ifd);
	if(slot == radix::NO_SLOT)
		return EBADF;
	// Linux's watch numbers are positive `int`s.
	if(wd < 0)
		return EINVAL;
	uint64_t status = radix_file_watch_remove(slot, static_cast<uint32_t>(wd));
	return status ? watch_errno(status) : 0;
}

} // namespace mlibc
