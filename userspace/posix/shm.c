// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Shared memory across processes (ADR 0490): POSIX shared memory objects
// in /dev/shm, which every process that maps one shares, by name or not,
// with their sizes, rights and SIGBUS past their ends; named and
// process-shared semaphores in them; and System V shared memory and
// semaphores, with Linux's keys, numbers, attachments, waits, undos and
// removals.

#include <dirent.h>
#include <fcntl.h>
#include <semaphore.h>
#include <stdint.h>
#include <sys/ipc.h>
#include <sys/mman.h>
#include <sys/sem.h>
#include <sys/shm.h>
#include <sys/stat.h>
#include <sys/statvfs.h>

#include "check.h"

#define PAGE 4096
#define NAME "/posix-shm-test"

union semun {
	int val;
	struct semid_ds *buf;
	unsigned short *array;
};

// Whether /dev/shm lists `name`.
static int listed(const char *name) {
	DIR *dir = opendir("/dev/shm");
	if(!dir)
		return 0;
	int found = 0;
	struct dirent *entry;
	while((entry = readdir(dir)))
		found |= !strcmp(entry->d_name, name);
	closedir(dir);
	return found;
}

// A child's status once it was killed by `sig`.
static int killed_by(int status, int sig) {
	return WIFSIGNALED(status) && WTERMSIG(status) == sig;
}

// An object made in one process is the same memory in another that opens
// it by name: what either writes, through a mapping or the descriptor,
// the other reads, and it stays while mapped after its name is gone.
static void objects(void) {
	shm_unlink(NAME);
	int fd = shm_open(NAME, O_RDWR | O_CREAT | O_EXCL, 0600);
	CHECK(fd >= 0);
	CHECK_ERROR(shm_open(NAME, O_RDWR | O_CREAT | O_EXCL, 0600), EEXIST);
	CHECK(listed("posix-shm-test"));
	struct stat st;
	CHECK(fstat(fd, &st) == 0 && st.st_size == 0 && (st.st_mode & 0777) == 0600);
	CHECK(ftruncate(fd, 3 * PAGE + 100) == 0);
	CHECK(fstat(fd, &st) == 0 && st.st_size == 3 * PAGE + 100);
	char *map = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	CHECK(map != MAP_FAILED);
	if(map == MAP_FAILED)
		return;
	CHECK(map[0] == 0 && map[3 * PAGE + 99] == 0);
	strcpy(map, "parent");
	strcpy(map + 2 * PAGE, "far");
	pid_t pid = check_fork();
	if(pid == 0) {
		// Its own mapping, by name, not the one fork copied.
		CHECK(munmap(map, 4 * PAGE) == 0);
		int own = shm_open(NAME, O_RDWR, 0);
		CHECK(own >= 0);
		char *mine = mmap(NULL, 3 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, own, 0);
		CHECK(mine != MAP_FAILED);
		if(mine != MAP_FAILED) {
			CHECK(!strcmp(mine, "parent") && !strcmp(mine + 2 * PAGE, "far"));
			strcpy(mine + PAGE, "child");
		}
		CHECK(pwrite(own, "written", 8, 2 * PAGE + 16) == 8);
		// A mapping from an offset into it.
		char *part = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, own, 2 * PAGE);
		CHECK(part != MAP_FAILED && !strcmp(part, "far") && !strcmp(part + 16, "written"));
		check_exit_child();
	}
	CHECK_CHILD(pid);
	CHECK(!strcmp(map + PAGE, "child"));
	CHECK(!strcmp(map + 2 * PAGE + 16, "written"));
	char buf[8] = {0};
	CHECK(pread(fd, buf, 7, 0) == 7 && !strcmp(buf, "parent"));
	CHECK(shm_unlink(NAME) == 0);
	CHECK(!listed("posix-shm-test"));
	CHECK_ERROR(shm_open(NAME, O_RDWR, 0), ENOENT);
	strcpy(map + PAGE, "after");
	CHECK(!strcmp(map + PAGE, "after"));
	CHECK(munmap(map, 4 * PAGE) == 0);
	close(fd);
	// Names with a slash inside, or none, are not names.
	CHECK_ERROR(shm_open("/a/b", O_RDWR | O_CREAT, 0600), EINVAL);
	CHECK_ERROR(shm_open("/", O_RDWR | O_CREAT, 0600), EINVAL);
	struct statvfs vfs;
	CHECK(statvfs("/dev/shm", &vfs) == 0 && vfs.f_blocks > 0);
}

// A mapping past its object's end gets SIGBUS, on Linux's page rule: the
// pages the object reaches into can be read, the rest cannot; shrinking
// an object takes the pages past its new end away from every mapping, and
// growing it again gives zeros there.
static void sizes(void) {
	shm_unlink(NAME);
	int fd = shm_open(NAME, O_RDWR | O_CREAT | O_EXCL, 0600);
	CHECK(fd >= 0 && ftruncate(fd, 2 * PAGE) == 0);
	char *map = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	CHECK(map != MAP_FAILED);
	if(map == MAP_FAILED)
		return;
	memset(map, 'x', 2 * PAGE);
	pid_t pid = check_fork();
	if(pid == 0) {
		signal(SIGBUS, SIG_DFL);
		(void)*(volatile char *)(map + 2 * PAGE);
		_exit(0);
	}
	CHECK(killed_by(check_wait(pid), SIGBUS));
	// Shrunk to the middle of its first page, while mapped here.
	CHECK(ftruncate(fd, 100) == 0);
	pid = check_fork();
	if(pid == 0) {
		signal(SIGBUS, SIG_DFL);
		CHECK(map[99] == 'x');
		(void)*(volatile char *)(map + PAGE);
		_exit(0);
	}
	CHECK(killed_by(check_wait(pid), SIGBUS));
	CHECK(ftruncate(fd, 2 * PAGE) == 0);
	CHECK(map[99] == 'x' && map[100] == 0 && map[PAGE - 1] == 0 && map[PAGE] == 0);
	CHECK(munmap(map, 4 * PAGE) == 0);
	close(fd);
	CHECK(shm_unlink(NAME) == 0);
}

// A mapping writes only what the descriptor may: one opened for reading
// alone maps for reading alone, and mprotect cannot make it writable.
// Other files' pages are the process's own copy, so a writable shared
// mapping of one fails with ENODEV, and a read-only one works.
static void rights(void) {
	shm_unlink(NAME);
	int fd = shm_open(NAME, O_RDWR | O_CREAT | O_EXCL, 0600);
	CHECK(fd >= 0 && ftruncate(fd, PAGE) == 0);
	CHECK(pwrite(fd, "data", 4, 0) == 4);
	int ro = shm_open(NAME, O_RDONLY, 0);
	CHECK(ro >= 0);
	CHECK(mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, ro, 0) == MAP_FAILED
			&& errno == EACCES);
	char *map = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, ro, 0);
	CHECK(map != MAP_FAILED && !memcmp(map, "data", 4));
	CHECK_ERROR(mprotect(map, PAGE, PROT_READ | PROT_WRITE), EACCES);
	// A private mapping is a copy, writable without the write right.
	char *copy = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, ro, 0);
	CHECK(copy != MAP_FAILED && !memcmp(copy, "data", 4));
	if(copy != MAP_FAILED)
		copy[0] = 'D';
	CHECK(map[0] == 'd');
	close(ro);
	close(fd);
	CHECK(shm_unlink(NAME) == 0);

	int file = open("/tmp/posix-shm-file", O_RDWR | O_CREAT | O_TRUNC, 0600);
	CHECK(file >= 0 && write(file, "file", 4) == 4);
	CHECK(mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, file, 0) == MAP_FAILED
			&& errno == ENODEV);
	char *read_only = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, file, 0);
	CHECK(read_only != MAP_FAILED && !memcmp(read_only, "file", 4));
	close(file);
	unlink("/tmp/posix-shm-file");
}

// A semaphore in shared memory wakes a waiter in another process: one
// sem_init made process-shared in an object, and a named one, which a
// process maps once however often it opens it.
static void semaphores(void) {
	shm_unlink(NAME);
	int fd = shm_open(NAME, O_RDWR | O_CREAT | O_EXCL, 0600);
	CHECK(fd >= 0 && ftruncate(fd, PAGE) == 0);
	sem_t *shared = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	CHECK(shared != MAP_FAILED);
	close(fd);
	if(shared == MAP_FAILED)
		return;
	CHECK(sem_init(shared, 1, 0) == 0);
	pid_t pid = check_fork();
	if(pid == 0) {
		CHECK(sem_wait(shared) == 0);
		check_exit_child();
	}
	check_sleep_ms(50);
	CHECK(sem_post(shared) == 0);
	CHECK_CHILD(pid);
	CHECK(shm_unlink(NAME) == 0);

	const char *name = "/posix-shm-sem";
	sem_unlink(name);
	sem_t *sem = sem_open(name, O_CREAT | O_EXCL, 0600, 0);
	CHECK(sem != SEM_FAILED);
	if(sem == SEM_FAILED)
		return;
	CHECK(sem_open(name, O_CREAT | O_EXCL, 0600, 0) == SEM_FAILED && errno == EEXIST);
	CHECK(listed("sem.posix-shm-sem"));
	CHECK(sem_open(name, 0) == sem);
	pid = check_fork();
	if(pid == 0) {
		// Opened by name in a fresh process image would be the same; here
		// the child closes what it inherited and opens it again.
		CHECK(sem_close(sem) == 0 && sem_close(sem) == 0);
		sem_t *again = sem_open(name, 0);
		CHECK(again != SEM_FAILED);
		if(again != SEM_FAILED)
			CHECK(sem_post(again) == 0);
		check_exit_child();
	}
	CHECK(sem_wait(sem) == 0);
	CHECK_CHILD(pid);
	int value = -1;
	CHECK(sem_getvalue(sem, &value) == 0 && value == 0);
	CHECK(sem_unlink(name) == 0);
	CHECK(sem_open(name, 0) == SEM_FAILED && errno == ENOENT);
	// Still mapped until closed as often as opened.
	CHECK(sem_close(sem) == 0 && sem_post(sem) == 0 && sem_close(sem) == 0);
}

// System V segments: found by key or made private, attached in each
// process that maps one and counted, kept after removal until the last
// detach, with Linux's errors.
static void segments(void) {
	key_t key = ftok("/system/tests/posix", 'S');
	CHECK(key != -1);
	int old = shmget(key, 0, 0);
	if(old >= 0)
		shmctl(old, IPC_RMID, NULL);
	int id = shmget(key, 3 * PAGE, IPC_CREAT | IPC_EXCL | 0640);
	CHECK(id >= 0);
	CHECK_ERROR(shmget(key, PAGE, IPC_CREAT | IPC_EXCL | 0640), EEXIST);
	CHECK(shmget(key, 0, 0) == id && shmget(key, PAGE, 0) == id);
	CHECK_ERROR(shmget(key, 4 * PAGE, 0), EINVAL);
	CHECK_ERROR(shmget(IPC_PRIVATE, 0, IPC_CREAT | 0600), EINVAL);
	struct shmid_ds ds;
	CHECK(shmctl(id, IPC_STAT, &ds) == 0);
	CHECK(ds.shm_segsz == 3 * PAGE && ds.shm_nattch == 0 && ds.shm_cpid == getpid());
	CHECK((ds.shm_perm.mode & 0777) == 0640 && ds.shm_perm.uid == geteuid());
	CHECK(ds.shm_perm.__ipc_perm_key == key);
	ds.shm_perm.mode = 0600;
	CHECK(shmctl(id, IPC_SET, &ds) == 0);
	CHECK(shmctl(id, IPC_STAT, &ds) == 0 && (ds.shm_perm.mode & 0777) == 0600);

	char *at = shmat(id, NULL, 0);
	CHECK(at != (void *)-1);
	if(at == (void *)-1)
		return;
	strcpy(at, "segment");
	CHECK(shmctl(id, IPC_STAT, &ds) == 0 && ds.shm_nattch == 1 && ds.shm_lpid == getpid());
	pid_t pid = check_fork();
	if(pid == 0) {
		// fork copies the attachment, which counts.
		struct shmid_ds mine;
		CHECK(shmctl(id, IPC_STAT, &mine) == 0 && mine.shm_nattch == 2);
		CHECK(shmdt(at) == 0);
		char *ro = shmat(id, NULL, SHM_RDONLY);
		CHECK(ro != (void *)-1 && !strcmp(ro, "segment"));
		// A second one, at an address of its own.
		char *rw = shmat(id, NULL, 0);
		CHECK(rw != (void *)-1 && rw != ro);
		if(rw != (void *)-1)
			strcpy(rw + PAGE, "child");
		CHECK(shmctl(id, IPC_STAT, &mine) == 0 && mine.shm_nattch == 3);
		// Ends with both attached: exit detaches them.
		check_exit_child();
	}
	CHECK_CHILD(pid);
	CHECK(!strcmp(at + PAGE, "child"));
	CHECK(shmctl(id, IPC_STAT, &ds) == 0 && ds.shm_nattch == 1);

	// At a fixed address: only SHM_REMAP replaces what is there.
	CHECK_ERROR((long)shmat(id, at, 0), EINVAL);
	CHECK(shmat(id, at, SHM_REMAP) == at);
	CHECK(shmat(id, at + 100, SHM_RND | SHM_REMAP) == at);
	CHECK_ERROR((long)shmat(id, at + 100, 0), EINVAL);
	CHECK_ERROR(shmdt(at + PAGE), EINVAL);
	CHECK(shmctl(id, IPC_STAT, &ds) == 0 && ds.shm_nattch == 1);

	// Removed while attached: the key is free, the segment stays.
	CHECK(shmctl(id, IPC_RMID, NULL) == 0);
	CHECK_ERROR(shmget(key, 0, 0), ENOENT);
	CHECK(shmctl(id, IPC_STAT, &ds) == 0 && (ds.shm_perm.mode & SHM_DEST));
	CHECK(ds.shm_perm.__ipc_perm_key == IPC_PRIVATE);
	CHECK(!strcmp(at, "segment"));
	CHECK(shmdt(at) == 0);
	CHECK_ERROR(shmctl(id, IPC_STAT, &ds), EINVAL);
	CHECK_ERROR((long)shmat(id, NULL, 0), EINVAL);

	// A private one, seen by a forked child through its attachment, and
	// gone with its last detach once removed.
	int private = shmget(IPC_PRIVATE, 10000, IPC_CREAT | 0600);
	CHECK(private >= 0 && private != id);
	char *p = shmat(private, NULL, 0);
	CHECK(p != (void *)-1);
	CHECK(shmctl(private, IPC_RMID, NULL) == 0);
	pid = check_fork();
	if(pid == 0) {
		strcpy(p + 9000, "late");
		check_exit_child();
	}
	CHECK_CHILD(pid);
	CHECK(!strcmp(p + 9000, "late"));
	CHECK(shmdt(p) == 0);
	CHECK_ERROR(shmctl(private, IPC_STAT, &ds), EINVAL);
}

static int alarmed;

static void on_alarm(int sig) {
	(void)sig;
	alarmed = 1;
}

// Waits until semaphore `number` of set `id` has `count` waiters of the
// kind `cmd` (GETNCNT or GETZCNT) counts.
static int waiters(int id, int number, int cmd, int count) {
	for(int i = 0; i < 500; i++) {
		if(semctl(id, number, cmd) == count)
			return 1;
		check_sleep_ms(10);
	}
	return 0;
}

// System V semaphores: a set's values move all at once or not at all,
// waiters in other processes wake when they can go on, time out, are
// interrupted or see the set removed, and SEM_UNDO's adjustments are made
// when a process ends.
static void sets(void) {
	int id = semget(IPC_PRIVATE, 3, IPC_CREAT | 0600);
	CHECK(id >= 0);
	CHECK_ERROR(semget(IPC_PRIVATE, 0, IPC_CREAT | 0600), EINVAL);
	struct semid_ds ds;
	union semun arg = {.buf = &ds};
	CHECK(semctl(id, 0, IPC_STAT, arg) == 0 && ds.sem_nsems == 3 && ds.sem_otime == 0);
	unsigned short values[3] = {1, 0, 2};
	arg.array = values;
	CHECK(semctl(id, 0, SETALL, arg) == 0);
	unsigned short got[3] = {9, 9, 9};
	arg.array = got;
	CHECK(semctl(id, 0, GETALL, arg) == 0 && got[0] == 1 && got[1] == 0 && got[2] == 2);
	CHECK(semctl(id, 2, GETVAL) == 2);
	arg.val = 40000;
	CHECK_ERROR(semctl(id, 0, SETVAL, arg), ERANGE);
	CHECK_ERROR(semctl(id, 3, GETVAL), EINVAL);

	struct sembuf take = {0, -1, 0};
	CHECK(semop(id, &take, 1) == 0 && semctl(id, 0, GETVAL) == 0);
	CHECK(semctl(id, 0, GETPID) == getpid());
	CHECK(semctl(id, 0, IPC_STAT, (union semun){.buf = &ds}) == 0 && ds.sem_otime > 0);
	struct sembuf wait_one = {1, -1, IPC_NOWAIT};
	CHECK_ERROR(semop(id, &wait_one, 1), EAGAIN);
	// All or none: the first would go, the second cannot.
	struct sembuf both[2] = {{2, -1, 0}, {1, -1, IPC_NOWAIT}};
	CHECK_ERROR(semop(id, both, 2), EAGAIN);
	CHECK(semctl(id, 2, GETVAL) == 2);
	struct sembuf out_of_set = {3, 1, 0};
	CHECK_ERROR(semop(id, &out_of_set, 1), EFBIG);
	CHECK_ERROR(semop(id, &take, 0), EINVAL);
	struct sembuf many[501];
	for(int i = 0; i < 501; i++)
		many[i] = (struct sembuf){2, 0, IPC_NOWAIT};
	CHECK_ERROR(semop(id, many, 501), E2BIG);

	// A time limit.
	struct timespec limit = {0, 50 * 1000 * 1000};
	struct sembuf slow = {1, -1, 0};
	long long start = check_now_ns();
	CHECK_ERROR(semtimedop(id, &slow, 1, &limit), EAGAIN);
	CHECK(check_now_ns() - start >= 40 * 1000 * 1000);

	// A waiter in another process, woken by a post here.
	pid_t pid = check_fork();
	if(pid == 0) {
		struct sembuf wait = {1, -1, 0};
		CHECK(semop(id, &wait, 1) == 0);
		check_exit_child();
	}
	CHECK(waiters(id, 1, GETNCNT, 1));
	struct sembuf post = {1, 1, 0};
	CHECK(semop(id, &post, 1) == 0);
	CHECK_CHILD(pid);
	CHECK(semctl(id, 1, GETVAL) == 0 && semctl(id, 1, GETPID) == pid);

	// A wait for zero.
	pid = check_fork();
	if(pid == 0) {
		struct sembuf zero = {2, 0, 0};
		CHECK(semop(id, &zero, 1) == 0);
		check_exit_child();
	}
	CHECK(waiters(id, 2, GETZCNT, 1));
	struct sembuf drain = {2, -2, 0};
	CHECK(semop(id, &drain, 1) == 0);
	CHECK_CHILD(pid);

	// SEM_UNDO: what a process did is undone when it ends.
	pid = check_fork();
	if(pid == 0) {
		struct sembuf up = {0, 5, SEM_UNDO};
		CHECK(semop(id, &up, 1) == 0 && semctl(id, 0, GETVAL) == 5);
		check_exit_child();
	}
	CHECK_CHILD(pid);
	CHECK(semctl(id, 0, GETVAL) == 0);

	// A signal interrupts a wait, with EINTR whatever SA_RESTART says.
	pid = check_fork();
	if(pid == 0) {
		struct sigaction sa;
		memset(&sa, 0, sizeof(sa));
		sa.sa_handler = on_alarm;
		sa.sa_flags = SA_RESTART;
		sigaction(SIGALRM, &sa, NULL);
		alarm(1);
		struct sembuf wait = {1, -1, 0};
		CHECK_ERROR(semop(id, &wait, 1), EINTR);
		CHECK(alarmed);
		check_exit_child();
	}
	CHECK_CHILD(pid);

	// Removal wakes a waiter with EIDRM.
	pid = check_fork();
	if(pid == 0) {
		struct sembuf wait = {1, -1, 0};
		CHECK_ERROR(semop(id, &wait, 1), EIDRM);
		check_exit_child();
	}
	CHECK(waiters(id, 1, GETNCNT, 1));
	CHECK(semctl(id, 0, IPC_RMID) == 0);
	CHECK_CHILD(pid);
	CHECK_ERROR(semctl(id, 0, GETVAL), EINVAL);

	// A key finds the same set in another process.
	key_t key = ftok("/system/tests/posix", 's');
	int old = semget(key, 0, 0);
	if(old >= 0)
		semctl(old, 0, IPC_RMID);
	int keyed = semget(key, 1, IPC_CREAT | IPC_EXCL | 0600);
	CHECK(keyed >= 0);
	CHECK_ERROR(semget(key, 2, 0), EINVAL);
	pid = check_fork();
	if(pid == 0) {
		int found = semget(key, 1, 0);
		struct sembuf one = {0, 1, 0};
		CHECK(found == keyed && semop(found, &one, 1) == 0);
		check_exit_child();
	}
	CHECK_CHILD(pid);
	CHECK(semctl(keyed, 0, GETVAL) == 1);
	CHECK(semctl(keyed, 0, IPC_RMID) == 0);
}

int main(void) {
	check_start("shm");
	RUN(objects);
	RUN(sizes);
	RUN(rights);
	RUN(semaphores);
	RUN(segments);
	RUN(sets);
	return check_end();
}
