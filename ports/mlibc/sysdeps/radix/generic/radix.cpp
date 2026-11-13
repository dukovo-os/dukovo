// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The helpers the Radix system layer's files share (radix.hpp).

#include <abi-bits/auxv.h>
#include <abi-bits/fcntl.h>
#include <abi-bits/stat.h>
#include <bits/ensure.h>
#include <elf.h>
#include <errno.h>
#include <limits.h>
#include <string.h>
#include <sys/sysmacros.h>
#include <sys/uio.h>
#include <time.h>

#ifndef MLIBC_BUILDING_RTLD
#include <mlibc/tcb.hpp>
#include <mlibc/thread.hpp>
#include <mlibc/threads.hpp>
#endif

#include "radix.hpp"

// The table of taken descriptors (radix::FdTable): the dynamic linker's,
// which the shared C library uses, or a static program's own.
#if defined(MLIBC_BUILDING_RTLD) || defined(MLIBC_STATIC_BUILD)
// Hidden, so that the dynamic linker's own code reaches it with no
// relocation that names a symbol: it relocates itself before it can look
// one up, and only knows relocations that add its base. The same table
// goes to the shared C library under the name __radix_fd_table.
extern "C" [[gnu::visibility("hidden")]] radix::FdTable __radix_fd_table_here;
radix::FdTable __radix_fd_table_here;
extern "C" [[gnu::alias("__radix_fd_table_here"), gnu::visibility("default")]]
radix::FdTable __radix_fd_table;
#else
// The dynamic linker's, found when it relocates the C library.
extern "C" [[gnu::visibility("default")]] radix::FdTable __radix_fd_table;
#endif

namespace radix {

namespace {

// The working directory, absolute, with no `/` at its end but for the
// root. Threads share it: chdir is rare, and a path resolved while it
// changes uses the old one or the new one.
char current_dir[PATH_MAX_BYTES] = "/";

// The cached process number: 0 until asked.
uint64_t cached_pid;

// Each descriptor's next listing cursor (readdir): only directories use
// theirs. Untouched pages of it take no memory.
uint64_t cursors[MAX_FDS];

// <sys/auxv.h>'s numbers for Radix's own entries are the kernel's.
static_assert(AT_RADIX_CWD == RADIX_PROCESS_AUX_CWD);
static_assert(AT_RADIX_STARTUP == RADIX_PROCESS_AUX_STARTUP);

// The first thread's stack, which exec makes 8 MiB (radix-posix(7)), and
// its top: 0 until start-up finds it.
constexpr size_t FIRST_STACK_BYTES = 8 << 20;
uintptr_t first_stack_top;

} // namespace

int to_errno(uint64_t status) {
	if(!status)
		return 0;
	if(status == CANCELED)
		return ECANCELED;
	switch(RADIX_STATUS_DOMAIN(status)) {
	case RADIX_KERNEL_ERROR_DOMAIN:
		return radix_kernel_errno(status);
	case RADIX_FILE_ERROR_DOMAIN:
		return radix_file_errno(status);
	default:
		return EIO;
	}
}

namespace {

// The soft RLIMIT_NOFILE, once read (-1 before): only this program's
// setrlimit changes it, and exec reads it again.
int files_soft = -1;

} // namespace

int files_limit() {
	int limit = __atomic_load_n(&files_soft, __ATOMIC_RELAXED);
	if(limit >= 0)
		return limit;
	struct radix_process_limits_results r;
	if(radix_process_limits(SLOT_PROCESS, RADIX_LIMIT_KIND_FILES, false, 0, 0, &r))
		return MAX_FDS;
	set_files_limit(r.current);
	return __atomic_load_n(&files_soft, __ATOMIC_RELAXED);
}

void set_files_limit(uint64_t limit) {
	int fds = limit < static_cast<uint64_t>(MAX_FDS) ? static_cast<int>(limit) : MAX_FDS;
	__atomic_store_n(&files_soft, fds, __ATOMIC_RELAXED);
}

namespace {

// The table (radix.hpp's FdTable).
#if defined(MLIBC_BUILDING_RTLD) || defined(MLIBC_STATIC_BUILD)
constexpr FdTable &table = __radix_fd_table_here;
#else
constexpr FdTable &table = __radix_fd_table;
#endif

// The word of the table with descriptor `fd`'s bit, and the bit.
uint64_t *word_of(int fd) {
	return &table.taken[fd / 64];
}

uint64_t bit_of(int fd) {
	return UINT64_C(1) << (fd % 64);
}

} // namespace

void SlotClaim::keep_if_full() {
	// The 64 slots from the word's first: one call, on a path taken only
	// when another capability came first.
	int fd = static_cast<int>(slot_ - FD_BASE);
	struct radix_process_slots_used_results used;
	uint32_t first = FD_BASE + static_cast<uint32_t>(fd / 64 * 64);
	if(!radix_process_slots_used(SLOT_PROCESS, first, &used) && (used.used & bit_of(fd)))
		keep();
}

void fd_taken(int fd) {
	__atomic_fetch_or(word_of(fd), bit_of(fd), __ATOMIC_ACQUIRE);
}

void fd_freed(int fd) {
	__atomic_fetch_and(word_of(fd), ~bit_of(fd), __ATOMIC_RELEASE);
}

uint64_t delete_fd(int fd) {
	uint64_t status = radix_capability_delete(slot_of(fd));
	// Free whatever the kernel said: a slot it could not empty answers
	// `occupied` to the next call that fills it, which marks it again.
	fd_freed(fd);
	return status;
}

int close_fd(int fd) {
	// One call from close's system layer, which these are inlined into.
	forget_dir_path(fd);
	return to_errno(delete_fd(fd));
}

void close_taken(int first, int last, bool close_on_exec) {
	for(int base = first / 64 * 64; base <= last; base += 64) {
		uint64_t taken = __atomic_load_n(word_of(base), __ATOMIC_RELAXED);
		// Only the descriptors from `first` to `last`.
		if(base < first)
			taken &= ~(bit_of(first) - 1);
		if(last - base < 63)
			taken &= (bit_of(last) << 1) - 1;
		for(; taken; taken &= taken - 1) {
			int fd = base + __builtin_ctzll(taken);
			if(!close_on_exec) {
				close_fd(fd);
				continue;
			}
			struct radix_capability_flags_results old;
			radix_capability_flags(slot_of(fd), true, RADIX_SLOT_FLAGS_CLOSE_ON_EXEC, &old);
		}
	}
}

void read_fd_table() {
	for(uint64_t &word : table.taken)
		__atomic_store_n(&word, 0, __ATOMIC_RELAXED);
	for(int fd = 0; fd < MAX_FDS; fd += 64) {
		struct radix_process_slots_used_results used;
		uint32_t first = FD_BASE + static_cast<uint32_t>(fd);
		if(radix_process_slots_used(SLOT_PROCESS, first, &used))
			break;
		__atomic_store_n(&table.slots, used.slots, __ATOMIC_RELAXED);
		if(first >= used.slots)
			break;
		__atomic_store_n(word_of(fd), used.used, __ATOMIC_RELAXED);
	}
	__atomic_store_n(&table.read, true, __ATOMIC_RELAXED);
}

int reach_slot(uint32_t slot) {
	uint32_t slots = __atomic_load_n(&table.slots, __ATOMIC_RELAXED);
	if(slot < slots)
		return 0;
	if(slot >= MAX_SLOTS)
		return EMFILE;
	// Twice as many as it has, as many as it needs, at most MAX_SLOTS. A
	// space never shrinks: asking for fewer than it has (another thread
	// grew it) changes nothing.
	uint32_t want = slots * 2 > slot + 1 ? slots * 2 : slot + 1;
	if(want < RADIX_PROCESS_MIN_SLOTS)
		want = RADIX_PROCESS_MIN_SLOTS;
	if(want > MAX_SLOTS)
		want = MAX_SLOTS;
	struct radix_process_grow_results grown;
	if(uint64_t status = radix_process_grow(SLOT_PROCESS, want, &grown); status) {
		int e = to_errno(status);
		return e == ENOMEM ? EMFILE : e;
	}
	// What the space has now, which another thread's growing may have
	// made more than this asked for.
	__atomic_store_n(&table.slots, grown.slots, __ATOMIC_RELAXED);
	return slot < grown.slots ? 0 : EMFILE;
}

int free_slot(int min_fd, SlotClaim *claim) {
	if(min_fd < 0 || min_fd >= MAX_FDS)
		return EINVAL;
	// The dynamic linker's first open, before the C library has started.
	if(!__atomic_load_n(&table.read, __ATOMIC_RELAXED))
		read_fd_table();
	int limit = files_limit();
	for(int first = min_fd / 64 * 64; first < limit; first += 64) {
		uint64_t seen = __atomic_load_n(word_of(first), __ATOMIC_RELAXED);
		// The descriptors below min_fd count as taken.
		if(first < min_fd)
			seen |= bit_of(min_fd) - 1;
		while(~seen) {
			int fd = first + __builtin_ctzll(~seen);
			if(fd >= limit)
				return EMFILE;
			// Taken for this thread, unless another took it first. Only
			// the one bit is tested, which x86-64 does with one locked
			// bit-test-and-set rather than a compare-and-swap loop.
			if(__atomic_fetch_or(word_of(fd), bit_of(fd), __ATOMIC_ACQUIRE) & bit_of(fd)) {
				seen |= __atomic_load_n(word_of(fd), __ATOMIC_RELAXED) | bit_of(fd);
				continue;
			}
			claim->slot_ = FD_BASE + static_cast<uint32_t>(fd);
			forget_dir_path(fd);
			// A claim that cannot be had is free again as it goes.
			return reach_slot(claim->slot_);
		}
	}
	return EMFILE;
}

namespace {

// Appends the components of `path` to the resolved path in `out` (which
// holds `len` bytes, no `/` at either end), working out `.` and `..` by
// their names: only for a working directory, which has no links in it.
int append(char *out, size_t *len, const char *path) {
	const char *p = path;
	while(*p) {
		while(*p == '/')
			p++;
		const char *end = p;
		while(*end && *end != '/')
			end++;
		size_t n = end - p;
		if(n == 0 || (n == 1 && p[0] == '.')) {
			// Nothing, or this directory.
		} else if(n == 2 && p[0] == '.' && p[1] == '.') {
			// The parent: the root is its own.
			while(*len > 0 && out[*len - 1] != '/')
				(*len)--;
			if(*len > 0)
				(*len)--;
		} else {
			size_t need = *len + (*len ? 1 : 0) + n;
			if(need >= PATH_MAX_BYTES)
				return ENAMETOOLONG;
			if(*len)
				out[(*len)++] = '/';
			memcpy(out + *len, p, n);
			*len += n;
		}
		p = end;
	}
	return 0;
}

// Whether `path` has a `..` among its names.
bool climbs(const char *path, size_t len) {
	for(size_t i = 0; i < len; i++) {
		bool starts = i == 0 || path[i - 1] == '/';
		if(starts && path[i] == '.' && i + 1 < len && path[i + 1] == '.'
				&& (i + 2 == len || path[i + 2] == '/'))
			return true;
	}
	return false;
}

// Joins `rest`, a path beneath the directory whose path beneath the root
// is the `len` bytes at `out`, onto it: `.` stands for the directory
// itself in either. Returns the length, or 0 if it does not fit.
size_t join(char *out, size_t len, const char *rest, size_t rest_len) {
	if(rest_len == 1 && rest[0] == '.')
		return len;
	if(len == 1 && out[0] == '.')
		len = 0;
	size_t need = len + (len ? 1 : 0) + rest_len;
	if(need >= PATH_MAX_BYTES)
		return 0;
	if(len)
		out[len++] = '/';
	memcpy(out + len, rest, rest_len);
	out[need] = 0;
	return need;
}

} // namespace

int from_root(int dirfd, const char *path, size_t len, Path *out) {
	size_t dir = dir_path(dirfd, out->own);
	if(!dir)
		return EXDEV;
	out->dir = SLOT_ROOT;
	out->text = out->own;
	out->len = join(out->own, dir, path, len);
	return out->len ? 0 : ENAMETOOLONG;
}

int resolve(int dirfd, const char *path, Path *out) {
	if(!path)
		return EFAULT;
	if(!*path)
		return ENOENT;
	size_t total = strlen(path);
	if(total >= PATH_MAX_BYTES)
		return ENAMETOOLONG;
	if(path[0] != '/' && dirfd != AT_FDCWD) {
		// Beneath the directory `dirfd` holds, which the kernel resolves.
		out->dir = slot_of(dirfd);
		if(out->dir == NO_SLOT)
			return EBADF;
		// `..` cannot climb above that directory there, so a path with
		// one goes from the root, by the directory's path, when the C
		// library knows it (ADR 0089); one that climbs above a directory
		// without a path fails with EXDEV.
		if(climbs(path, total)) {
			if(int e = from_root(dirfd, path, total, out); e != EXDEV)
				return e;
		}
		// The kernel takes it as it is.
		out->text = path;
		out->len = total;
		return 0;
	}
	// Beneath the root: an absolute path without its leading slashes, or
	// the working directory (which has no links or `..` in it) and the
	// path. The kernel works out `.`, `..` and symbolic links, as a link
	// changes where a `..` after it leads.
	out->dir = SLOT_ROOT;
	if(path[0] != '/' && current_dir[1]) {
		size_t dir = strlen(current_dir + 1);
		if(dir + 1 + total >= PATH_MAX_BYTES)
			return ENAMETOOLONG;
		memcpy(out->own, current_dir + 1, dir);
		out->own[dir] = '/';
		memcpy(out->own + dir + 1, path, total + 1);
		out->text = out->own;
		out->len = dir + 1 + total;
		return 0;
	}
	// Already beneath the root, past its slashes: as it is. The root
	// itself is `.`.
	const char *rest = path;
	while(*rest == '/')
		rest++;
	out->len = total - static_cast<size_t>(rest - path);
	out->text = out->len ? rest : ".";
	if(!out->len)
		out->len = 1;
	return 0;
}

const char *cwd() {
	return current_dir;
}

int set_cwd(const char *path) {
	char resolved[PATH_MAX_BYTES];
	size_t len = 0;
	if(int e = append(resolved, &len, path); e)
		return e;
	if(len + 2 > PATH_MAX_BYTES)
		return ENAMETOOLONG;
	current_dir[0] = '/';
	memcpy(current_dir + 1, resolved, len);
	current_dir[len + 1] = 0;
	return 0;
}

void init_from_auxv(const uintptr_t *auxv) {
	const char *last = nullptr;
	for(; auxv[0] != AT_NULL; auxv += 2) {
		if(auxv[0] == AT_RADIX_CWD) {
			auto dir = reinterpret_cast<const char *>(auxv[1]);
			if(dir && dir[0] == '/')
				set_cwd(dir);
			last = dir;
		} else if(auxv[0] == AT_EXECFN && !last) {
			last = reinterpret_cast<const char *>(auxv[1]);
		}
	}
	// The working directory's string is the stack's last, or with none
	// the path the program was run as, and 8 zero bytes follow it to the
	// top, as on Linux.
	if(last)
		first_stack_top = reinterpret_cast<uintptr_t>(last) + strlen(last) + 1 + 8;
}

int first_stack(void **base, size_t *size) {
	if(!first_stack_top)
		return ENOSYS;
	*base = reinterpret_cast<void *>(first_stack_top - FIRST_STACK_BYTES);
	*size = FIRST_STACK_BYTES;
	return 0;
}

uint64_t pid() {
	uint64_t known = __atomic_load_n(&cached_pid, __ATOMIC_RELAXED);
	if(known)
		return known;
	struct radix_process_ids_results ids;
	if(radix_process_ids(SLOT_PROCESS, 0, &ids))
		return 0;
	__atomic_store_n(&cached_pid, ids.pid, __ATOMIC_RELAXED);
	return ids.pid;
}

void forget_pid() {
	__atomic_store_n(&cached_pid, 0, __ATOMIC_RELAXED);
}

void fill_stat(struct ::stat *st, const struct radix_file_status &f) {
	memset(st, 0, sizeof(*st));
	// A file's number is unique on its filesystem, and the mount's number
	// tells the filesystems apart.
	st->st_dev = f.mount;
	st->st_ino = f.id;
	switch(f.kind) {
	case RADIX_FILE_KIND_DIRECTORY:
		st->st_mode = S_IFDIR;
		break;
	case RADIX_FILE_KIND_BLOCK_DEVICE:
		st->st_mode = S_IFBLK;
		break;
	case RADIX_FILE_KIND_CHAR_DEVICE:
		st->st_mode = S_IFCHR;
		break;
	case RADIX_FILE_KIND_PIPE:
		st->st_mode = S_IFIFO;
		break;
	case RADIX_FILE_KIND_SYMLINK:
		st->st_mode = S_IFLNK;
		break;
	case RADIX_FILE_KIND_SOCKET:
		st->st_mode = S_IFSOCK;
		break;
	default:
		st->st_mode = S_IFREG;
		break;
	}
	// Its permission bits and owners, as its filesystem keeps them or
	// the kernel makes them up (ADR 0213).
	st->st_mode |= f.mode & 07777;
	st->st_uid = f.owner;
	st->st_gid = f.group;
	st->st_nlink = f.links;
	if(f.kind == RADIX_FILE_KIND_CHAR_DEVICE || f.kind == RADIX_FILE_KIND_BLOCK_DEVICE)
		st->st_rdev = makedev(static_cast<unsigned int>(f.device >> 32),
				static_cast<unsigned int>(f.device));
	st->st_size = f.size;
	st->st_blksize = 4096;
	st->st_blocks = (f.allocated + 511) / 512;
	auto at = [](uint64_t ns) {
		struct timespec ts;
		ts.tv_sec = ns / 1000000000;
		ts.tv_nsec = ns % 1000000000;
		return ts;
	};
	st->st_mtim = at(f.modified);
	st->st_ctim = at(f.changed);
	st->st_atim = at(f.accessed);
}

uint64_t dir_cursor(int fd) {
	if(fd < 0 || fd >= MAX_FDS)
		return 0;
	return __atomic_load_n(&cursors[fd], __ATOMIC_RELAXED);
}

void set_dir_cursor(int fd, uint64_t cursor) {
	if(fd >= 0 && fd < MAX_FDS)
		__atomic_store_n(&cursors[fd], cursor, __ATOMIC_RELAXED);
}

namespace {

// The paths of directory descriptors (ADR 0089), in a pool of entries a
// page each, of which only those that have been used take memory. They
// are taken and given back with atomic operations alone: close and dup2
// are async-signal-safe, and a child after fork calls them while a lock
// its parent's other threads held may stay held.
constexpr int DIR_PATHS = 64;
struct alignas(4096) DirPath {
	char text[PATH_MAX_BYTES];
};
DirPath dir_paths[DIR_PATHS];

// The entries in use, a bit each.
uint64_t dir_paths_used;
static_assert(DIR_PATHS <= 64);

// Each descriptor's entry, plus one: 0 for none.
uint8_t dir_path_of[MAX_FDS];

// Takes a free entry: its number, or -1 if all are in use.
int take_dir_path() {
	uint64_t used = __atomic_load_n(&dir_paths_used, __ATOMIC_RELAXED);
	for(;;) {
		if(!~used)
			return -1;
		int n = __builtin_ctzll(~used);
		if(__atomic_compare_exchange_n(&dir_paths_used, &used, used | (UINT64_C(1) << n), true,
				__ATOMIC_ACQUIRE, __ATOMIC_RELAXED))
			return n;
	}
}

// Gives entry `n` (-1 for none) back.
void give_dir_path(int n) {
	if(n >= 0)
		__atomic_fetch_and(&dir_paths_used, ~(UINT64_C(1) << n), __ATOMIC_RELEASE);
}

// Makes entry `n` (-1 for none) descriptor `fd`'s, giving back the one it
// had.
void set_dir_path(int fd, int n) {
	if(fd < 0 || fd >= MAX_FDS) {
		give_dir_path(n);
		return;
	}
	uint8_t old = __atomic_exchange_n(&dir_path_of[fd], static_cast<uint8_t>(n + 1),
			__ATOMIC_RELEASE);
	give_dir_path(static_cast<int>(old) - 1);
}

} // namespace

void note_dir_path(int fd, const Path &path) {
	int n = take_dir_path();
	if(n < 0) {
		set_dir_path(fd, -1);
		return;
	}
	char *text = dir_paths[n].text;
	size_t len = 1;
	text[0] = '.';
	text[1] = 0;
	if(path.dir != SLOT_ROOT) {
		// Beneath another directory descriptor, which resolve gave the
		// kernel as it was: its path, then this one.
		int from = static_cast<int>(path.dir - FD_BASE);
		if(path.dir < FD_BASE || !(len = dir_path(from, text))) {
			give_dir_path(n);
			set_dir_path(fd, -1);
			return;
		}
	}
	len = join(text, len, path.text, path.len);
	if(len && climbs(text, len)) {
		// Where it really is, so that the paths of the directories a walk
		// climbs back to do not grow with each `..`.
		struct radix_directory_real_path_results r;
		char canonical[PATH_MAX_BYTES];
		if(radix_directory_real_path(SLOT_ROOT, text, len, canonical, sizeof(canonical) - 1, &r)
				|| r.len >= sizeof(canonical)) {
			len = 0;
		} else {
			memcpy(text, canonical, r.len);
			text[r.len] = 0;
			len = r.len;
		}
	}
	if(!len) {
		give_dir_path(n);
		n = -1;
	}
	set_dir_path(fd, n);
}

void copy_dir_path(int from, int to) {
	if(from == to)
		return;
	int n = -1;
	if(from >= 0 && from < MAX_FDS && __atomic_load_n(&dir_path_of[from], __ATOMIC_RELAXED)
			&& (n = take_dir_path()) >= 0 && !dir_path(from, dir_paths[n].text)) {
		give_dir_path(n);
		n = -1;
	}
	set_dir_path(to, n);
}

void forget_dir_path(int fd) {
	if(fd >= 0 && fd < MAX_FDS && __atomic_load_n(&dir_path_of[fd], __ATOMIC_RELAXED))
		set_dir_path(fd, -1);
}

size_t dir_path(int fd, char *out) {
	if(fd < 0 || fd >= MAX_FDS)
		return 0;
	int n = static_cast<int>(__atomic_load_n(&dir_path_of[fd], __ATOMIC_ACQUIRE)) - 1;
	if(n < 0)
		return 0;
	size_t len = strlen(dir_paths[n].text);
	memcpy(out, dir_paths[n].text, len + 1);
	return len;
}

// The numbers are the same up to CLOCK_BOOTTIME. TAI reads as REALTIME,
// which it is until something sets the offset between them: nothing does
// on Radix, nor by default on Linux. The alarm clocks are EINVAL, as on a
// Linux machine without a clock that can wake it.
int clock_kind(int clock, enum radix_clock_kind *kind) {
	if(clock == CLOCK_TAI) {
		*kind = RADIX_CLOCK_KIND_REALTIME;
		return 0;
	}
	if(clock < 0 || clock > RADIX_CLOCK_KIND_BOOTTIME)
		return EINVAL;
	*kind = static_cast<enum radix_clock_kind>(clock);
	return 0;
}

namespace {

// The ids of CPU-time clocks are glibc's, which Linux's kernel decodes:
// the number of a process or thread, complemented, above three low bits
// that hold the clock's kind and whether it is a thread's. They are made
// as glibc's MAKE_PROCESS_CPUCLOCK and MAKE_THREAD_CPUCLOCK make them
// from the number and the kind CPUCLOCK_SCHED, the CPU time used.
constexpr int CPU_CLOCK_KIND_MASK = 3;
constexpr int CPU_CLOCK_SCHEDULED = 2;
constexpr int CPU_CLOCK_THREAD = 4;
// A number from here on does not fit: its id would not be negative, which
// is how a CPU-time clock is told from the others.
constexpr pid_t CPU_CLOCK_NUMBER_LIMIT = 1 << 28;

int make_cpu_clock(pid_t number, int bits, clockid_t *clock) {
	if(number < 0)
		return ESRCH;
	if(number >= CPU_CLOCK_NUMBER_LIMIT)
		return ENOENT;
	*clock = static_cast<clockid_t>((~static_cast<uint32_t>(number) << 3) | bits);
	return 0;
}

} // namespace

int thread_cpu_clock(pid_t tid, clockid_t *clock) {
	// No thread is numbered 0, which in a clock id is the calling thread:
	// a thread without a number has ended, as glibc's INVALID_TD_P says.
	if(tid <= 0)
		return ESRCH;
	return make_cpu_clock(tid, CPU_CLOCK_SCHEDULED | CPU_CLOCK_THREAD, clock);
}

int process_cpu_clock(pid_t pid, clockid_t *clock) {
	return make_cpu_clock(pid, CPU_CLOCK_SCHEDULED, clock);
}

int cpu_clock(clockid_t clock, uint64_t *id, bool *process) {
	if((clock & CPU_CLOCK_KIND_MASK) != CPU_CLOCK_SCHEDULED)
		return EINVAL;
	*process = !(clock & CPU_CLOCK_THREAD);
	*id = static_cast<uint64_t>(~(clock >> 3));
	return 0;
}

int iov_total(const struct iovec *iov, size_t count, size_t *sum) {
	size_t total = 0;
	for(size_t i = 0; i < count; i++) {
		if(iov[i].iov_len > static_cast<size_t>(SSIZE_MAX) - total)
			return EINVAL;
		total += iov[i].iov_len;
	}
	*sum = total;
	return 0;
}

void log(const char *text, size_t len) {
	radix_debug_log(SLOT_LOG, text, len);
}

} // namespace radix

#ifndef MLIBC_BUILDING_RTLD

extern "C" uint64_t __radix_cancellable_syscall(uint64_t number, const uint64_t in[10],
		uint64_t out[10], uint64_t masked);

// Acts on a cancel that is due (x86_64/cancel.S): ends the thread, or
// returns CANCELED for the call that is masked.
extern "C" [[gnu::visibility("hidden")]] uint64_t __radix_cancel(uint64_t masked) {
	if(masked)
		return radix::CANCELED;
	__mlibc_do_cancel();
	__builtin_unreachable();
}

namespace radix {

namespace {

uint64_t cancellable_call(uint64_t number, const uint64_t in[10], uint64_t out[10],
		uint64_t masked) {
	uint64_t status = __radix_cancellable_syscall(number, in, out, masked);
	// A signal interrupted the call before it did anything: a cancel that
	// came while it waited is acted on as one due before it.
	if(RADIX_STATUS_DOMAIN(status) == RADIX_KERNEL_ERROR_DOMAIN
			&& RADIX_STATUS_CODE(status) == RADIX_KERNEL_INTERRUPTED
			&& mlibc::tcb_cancelled(__atomic_load_n(&mlibc::get_current_tcb()->cancelBits,
					__ATOMIC_RELAXED)))
		return __radix_cancel(masked);
	return status;
}

} // namespace

uint64_t cancellable(uint64_t number, const uint64_t in[10], uint64_t out[10]) {
	return cancellable_call(number, in, out, 0);
}

uint64_t cancellable_masked(uint64_t number, const uint64_t in[10], uint64_t out[10]) {
	return cancellable_call(number, in, out, 1);
}

} // namespace radix

#endif // !MLIBC_BUILDING_RTLD
