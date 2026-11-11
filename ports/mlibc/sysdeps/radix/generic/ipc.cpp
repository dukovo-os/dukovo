// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// System V shared memory and semaphores on Radix (ADR 0490).
//
// The memory filesystem on /devices/shm keeps them: each call names it by
// its path beneath the root directory (SLOT_ROOT), so a program whose
// root has no /devices/shm, in a view, has none, and fails with ENOSYS
// as on a Linux without them. Their keys, numbers, permissions and
// semantics are Linux's (radix-directory(2)). Message queues are not
// there yet.

#include <errno.h>
#include <limits.h>
#include <mlibc/all-sysdeps.hpp>
#include <stdint.h>
#include <string.h>
#include <sys/ipc.h>
#include <sys/sem.h>
#include <sys/shm.h>
#include <time.h>

#include "radix.hpp"

namespace {

// The memory filesystem whose objects these are, beneath the root.
constexpr char IPC_PATH[] = "devices/shm";
constexpr size_t IPC_PATH_LEN = sizeof(IPC_PATH) - 1;

// The errno of `status`: no memory filesystem is a system without System
// V IPC.
int ipc_errno(uint64_t status) {
	if(RADIX_STATUS_DOMAIN(status) == RADIX_KERNEL_ERROR_DOMAIN
			&& RADIX_STATUS_CODE(status) == RADIX_KERNEL_NOT_SUPPORTED)
		return ENOSYS;
	return radix::to_errno(status);
}

// The flags of `ipc_get` for shmget's and semget's IPC_CREAT and IPC_EXCL.
uint32_t get_flags(int flags) {
	uint32_t out = 0;
	if(flags & IPC_CREAT)
		out |= RADIX_IPC_FLAGS_CREATE;
	if(flags & IPC_EXCL)
		out |= RADIX_IPC_FLAGS_EXCLUSIVE;
	return out;
}

int status_of(enum radix_ipc_kind kind, int id, struct radix_ipc_status *out) {
	if(id < 0)
		return EINVAL;
	return ipc_errno(radix_directory_ipc_status(radix::SLOT_ROOT, IPC_PATH, IPC_PATH_LEN, kind,
			static_cast<uint32_t>(id), out, sizeof(*out)));
}

// Linux's struct ipc_perm from an object's status.
void fill_perm(struct ipc_perm *perm, const struct radix_ipc_status &s) {
	memset(perm, 0, sizeof(*perm));
	perm->__ipc_perm_key = s.key;
	perm->uid = s.owner;
	perm->gid = s.group;
	perm->cuid = s.creator;
	perm->cgid = s.creator_group;
	perm->mode = s.mode;
	// A number is its sequence times 32768 plus its index, as on Linux.
	perm->__ipc_perm_seq = static_cast<int>(s.id / 32768);
}

// Seconds since 1970 of the kernel's nanoseconds.
time_t seconds(uint64_t ns) {
	return static_cast<time_t>(ns / 1000000000);
}

// IPC_SET: the owner, group and permission bits of `perm`.
int set_perm(enum radix_ipc_kind kind, int id, const struct ipc_perm *perm) {
	if(id < 0)
		return EINVAL;
	return ipc_errno(radix_directory_ipc_set(radix::SLOT_ROOT, IPC_PATH, IPC_PATH_LEN, kind,
			static_cast<uint32_t>(id), perm->uid, perm->gid, perm->mode & 0777));
}

int remove_object(enum radix_ipc_kind kind, int id) {
	if(id < 0)
		return EINVAL;
	return ipc_errno(radix_directory_ipc_remove(radix::SLOT_ROOT, IPC_PATH, IPC_PATH_LEN, kind,
			static_cast<uint32_t>(id)));
}

} // namespace

namespace mlibc {

int Sysdeps<Shmget>::operator()(int *shm_id, key_t key, size_t size, int shmflg) {
	struct radix_directory_ipc_get_results r;
	uint64_t status = radix_directory_ipc_get(radix::SLOT_ROOT, IPC_PATH, IPC_PATH_LEN,
			RADIX_IPC_KIND_SEGMENT, key, size, get_flags(shmflg),
			static_cast<uint32_t>(shmflg & 0777), &r);
	if(status)
		return ipc_errno(status);
	*shm_id = static_cast<int>(r.id);
	return 0;
}

int Sysdeps<Shmat>::operator()(void **seg_start, int shmid, const void *shmaddr, int shmflg) {
	if(shmid < 0)
		return EINVAL;
	uint64_t address = reinterpret_cast<uint64_t>(shmaddr);
	if(address && (shmflg & SHM_RND))
		address &= ~uint64_t{4095};
	if((address & 4095) || ((shmflg & SHM_REMAP) && !address))
		return EINVAL;
	uint32_t flags = 0;
	if(shmflg & SHM_RDONLY)
		flags |= RADIX_ATTACH_FLAGS_READ_ONLY;
	if(shmflg & SHM_EXEC)
		flags |= RADIX_ATTACH_FLAGS_EXECUTE;
	if(shmflg & SHM_REMAP)
		flags |= RADIX_ATTACH_FLAGS_REPLACE;
	struct radix_directory_ipc_attach_results r;
	uint64_t status = radix_directory_ipc_attach(radix::SLOT_ROOT, IPC_PATH, IPC_PATH_LEN,
			static_cast<uint32_t>(shmid), radix::SLOT_SPACE, address, flags, &r);
	if(status) {
		int e = ipc_errno(status);
		// Something mapped there already, which only SHM_REMAP replaces.
		return e == EEXIST ? EINVAL : e;
	}
	*seg_start = reinterpret_cast<void *>(r.address);
	return 0;
}

int Sysdeps<Shmdt>::operator()(const void *shmaddr) {
	return radix::to_errno(radix_address_space_detach(radix::SLOT_SPACE,
			reinterpret_cast<uint64_t>(shmaddr)));
}

int Sysdeps<Shmctl>::operator()(int *idx, int shmid, int cmd, struct shmid_ds *buf) {
	*idx = 0;
	if((cmd == IPC_STAT || cmd == IPC_SET) && !buf)
		return EFAULT;
	switch(cmd) {
	case IPC_STAT: {
		struct radix_ipc_status s;
		if(int e = status_of(RADIX_IPC_KIND_SEGMENT, shmid, &s); e)
			return e;
		memset(buf, 0, sizeof(*buf));
		fill_perm(&buf->shm_perm, s);
		buf->shm_segsz = s.size;
		buf->shm_atime = seconds(s.used);
		buf->shm_dtime = seconds(s.detached);
		buf->shm_ctime = seconds(s.changed);
		buf->shm_cpid = static_cast<pid_t>(s.creator_pid);
		buf->shm_lpid = static_cast<pid_t>(s.last_pid);
		buf->shm_nattch = s.attached;
		return 0;
	}
	case IPC_SET:
		return set_perm(RADIX_IPC_KIND_SEGMENT, shmid, &buf->shm_perm);
	case IPC_RMID:
		return remove_object(RADIX_IPC_KIND_SEGMENT, shmid);
	case SHM_LOCK:
	case SHM_UNLOCK: {
		// Segments' pages stay in memory: there is nothing to lock, but
		// the segment has to be there.
		struct radix_ipc_status s;
		return status_of(RADIX_IPC_KIND_SEGMENT, shmid, &s);
	}
	default:
		// IPC_INFO, SHM_INFO, SHM_STAT and SHM_STAT_ANY are not there yet.
		return EINVAL;
	}
}

int Sysdeps<Semget>::operator()(key_t key, int n, int fl, int *id) {
	if(n < 0)
		return EINVAL;
	struct radix_directory_ipc_get_results r;
	uint64_t status = radix_directory_ipc_get(radix::SLOT_ROOT, IPC_PATH, IPC_PATH_LEN,
			RADIX_IPC_KIND_SEMAPHORES, key, static_cast<uint64_t>(n), get_flags(fl),
			static_cast<uint32_t>(fl & 0777), &r);
	if(status)
		return ipc_errno(status);
	*id = static_cast<int>(r.id);
	return 0;
}

int Sysdeps<Semctl>::operator()(int semid, int semnum, int cmd, void *semun, int *ret) {
	*ret = 0;
	if(semid < 0)
		return EINVAL;
	uint32_t id = static_cast<uint32_t>(semid);
	auto control = [&](enum radix_semaphore_command command, uint32_t value) {
		if(semnum < 0)
			return EINVAL;
		struct radix_directory_semaphore_control_results r;
		uint64_t status = radix_directory_semaphore_control(radix::SLOT_ROOT, IPC_PATH,
				IPC_PATH_LEN, id, static_cast<uint32_t>(semnum), command, value, &r);
		if(status)
			return ipc_errno(status);
		*ret = static_cast<int>(r.value);
		return 0;
	};
	if((cmd == IPC_STAT || cmd == IPC_SET) && !semun)
		return EFAULT;
	switch(cmd) {
	case IPC_STAT: {
		struct radix_ipc_status s;
		if(int e = status_of(RADIX_IPC_KIND_SEMAPHORES, semid, &s); e)
			return e;
		auto out = static_cast<struct semid_ds *>(semun);
		memset(out, 0, sizeof(*out));
		fill_perm(&out->sem_perm, s);
		out->sem_otime = seconds(s.used);
		out->sem_ctime = seconds(s.changed);
		out->sem_nsems = s.size;
		return 0;
	}
	case IPC_SET:
		return set_perm(RADIX_IPC_KIND_SEMAPHORES, semid,
				&static_cast<struct semid_ds *>(semun)->sem_perm);
	case IPC_RMID:
		return remove_object(RADIX_IPC_KIND_SEMAPHORES, semid);
	case GETVAL:
		return control(RADIX_SEMAPHORE_COMMAND_GET_VALUE, 0);
	case SETVAL: {
		// The union semun's int, which the caller passed in the
		// pointer's place.
		int value = static_cast<int>(reinterpret_cast<uintptr_t>(semun));
		if(value < 0)
			return ERANGE;
		return control(RADIX_SEMAPHORE_COMMAND_SET_VALUE, static_cast<uint32_t>(value));
	}
	case GETPID:
		return control(RADIX_SEMAPHORE_COMMAND_GET_PID, 0);
	case GETNCNT:
		return control(RADIX_SEMAPHORE_COMMAND_GET_WAITING, 0);
	case GETZCNT:
		return control(RADIX_SEMAPHORE_COMMAND_GET_ZERO_WAITING, 0);
	case GETALL:
	case SETALL: {
		// The set's size first, which the array has room for.
		struct radix_directory_semaphore_values_results r;
		uint64_t status = radix_directory_semaphore_values(radix::SLOT_ROOT, IPC_PATH,
				IPC_PATH_LEN, id, nullptr, 0, &r);
		if(status)
			return ipc_errno(status);
		size_t len = static_cast<size_t>(r.count) * sizeof(unsigned short);
		if(cmd == SETALL)
			return ipc_errno(radix_directory_set_semaphore_values(radix::SLOT_ROOT, IPC_PATH,
					IPC_PATH_LEN, id, semun, len));
		return ipc_errno(radix_directory_semaphore_values(radix::SLOT_ROOT, IPC_PATH,
				IPC_PATH_LEN, id, semun, len, &r));
	}
	default:
		// IPC_INFO, SEM_INFO, SEM_STAT and SEM_STAT_ANY are not there yet.
		return EINVAL;
	}
}

int Sysdeps<Semtimedop>::operator()(int semid, struct sembuf *sops, size_t nsops,
		const struct timespec *timeout) {
	if(semid < 0 || !nsops)
		return EINVAL;
	if(nsops > SIZE_MAX / sizeof(struct sembuf))
		return E2BIG;
	// No time is no limit; a time of zero tries once, which the kernel
	// takes as one nanosecond, as 0 there is none.
	uint64_t ns = 0;
	if(timeout) {
		if(timeout->tv_sec < 0 || timeout->tv_nsec < 0 || timeout->tv_nsec >= 1000000000)
			return EINVAL;
		uint64_t secs = static_cast<uint64_t>(timeout->tv_sec);
		if(secs >= UINT64_MAX / 1000000000 - 1)
			ns = UINT64_MAX;
		else
			ns = secs * 1000000000 + static_cast<uint64_t>(timeout->tv_nsec);
		if(!ns)
			ns = 1;
	}
	// struct sembuf is the kernel's struct radix_semaphore_operation.
	static_assert(sizeof(struct sembuf) == sizeof(struct radix_semaphore_operation));
	return ipc_errno(radix_directory_semaphore_operate(radix::SLOT_ROOT, IPC_PATH, IPC_PATH_LEN,
			static_cast<uint32_t>(semid), sops, nsops * sizeof(struct sembuf), ns));
}

} // namespace mlibc
