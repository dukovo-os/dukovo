<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0490. Shared memory is a filesystem, which keeps System V IPC too

- Status: Accepted
- Date: 2026-11-11

## Context

The 1.0 server services need memory that processes share by name.
PostgreSQL keeps its buffers in a POSIX shared memory object or a
System V segment, and locks with System V semaphores; nginx, PHP-FPM
and Valkey share memory between a master and its workers; Python's
`multiprocessing.shared_memory` and named semaphores are POSIX ones.
Until now Radix had shared anonymous memory, which a forked child
shares, but no `/dev/shm`: a file's mapping was the process's own copy,
so `shm_open` gave an ordinary file that two processes could not share,
`sem_open` failed with ENOSYS, and `shmget`, `semget` and `semop` were
not there. libc-test's `ipc_shm`, `ipc_sem`, `sem_open` and
`sem_close-unmap`, and the `shm_open` case of `pthread_cancel-points`,
were left out for it.

On Linux, `/dev/shm` is a tmpfs, and System V objects live in an IPC
namespace beside it, whose keys every process in the namespace reaches.
Radix has no namespaces: a program reaches what its capabilities lead
to, and a service runs in a view of what its file declares (ADR 0217).

## Decision

- **A memory filesystem.** A new filesystem kind, `shm`, holds memory
  files: each one's pages are the same shared pages a shared anonymous
  mapping has, so every process that maps a file shared maps the same
  memory, whether it opened the file by name or inherited it. Spark
  mounts one on `/devices/shm` (`/dev/shm` through the `dev` link), mode
  1777, with room for half the machine's memory, as Linux's tmpfs there.
  It is flat: it holds files and hard links, which musl's `sem_open`
  makes, but no folders or symbolic links. Reading and writing a file
  copies through its pages, so `read`, `write` and a mapping always
  agree.
- **Mappings follow the file's size, as Linux's.** A file's pages end at
  its size, rounded up to a page; a mapping past that gets SIGBUS.
  `ftruncate` to a smaller size takes the pages past the new end from
  every process that maps them, and zeroes the rest of the last page,
  so growing the file again shows zeros there. A file opened for reading
  alone maps for reading alone, and `mprotect` cannot make that mapping
  writable (EACCES).
- **Other files are not shared yet.** The page cache's pages cannot be
  mapped into a process, so a writable shared mapping of any other file
  fails with ENODEV, as before, now from the kernel; a read-only one is
  the process's copy, which stays read-only.
- **System V shared memory and semaphores belong to a memory
  filesystem.** Each `shm` mount keeps a table of segments and one of
  semaphore sets, with Linux's keys, numbers (a sequence times 32768
  plus an index), permissions, limits and errors. The C library reaches
  them by the path `/devices/shm` beneath the process's root directory
  (radix-directory(2)'s `ipc_get` and the calls after it), so a
  program in a view without `/devices/shm` has no System V IPC, and
  fails with ENOSYS as on a Linux built without it; two views with
  their own memory filesystems would have separate keys, as two IPC
  namespaces do. Permissions are checked against the name the root
  capability acts for; its bypass right is Linux's `CAP_IPC_OWNER`.
- **Segments are memory files without names.** `shmat` maps a segment's
  shared pages and counts the mapping as an attachment, which `fork`
  copies and `shmdt`, `munmap` or the process's end take away;
  `IPC_RMID` frees the key at once and the segment with its last
  attachment, as on Linux. Segments take their room from their
  filesystem's.
- **Semaphores are Linux's.** `semop` and `semtimedop` do all of their
  operations at once or none, in order, waiting while one cannot go on;
  a signal ends the wait with EINTR, whatever SA_RESTART says; removal
  ends it with EIDRM; a time limit with EAGAIN. SEM_UNDO's adjustments
  are kept per process and made when it ends, clamped to the range a
  semaphore holds. The rules are in `radix-posix`'s `ipc` module, where
  host tests and a property test check them.
- **Named semaphores are musl's.** `sem_open` makes `/dev/shm/sem.NAME`
  (glibc's name) whole under a temporary name, links it to its own, and
  maps it shared; a process maps each one once, however often it opens
  it (mlibc patch 0070).

## Consequences

- PostgreSQL, nginx, Valkey, PHP-FPM and Python find the shared memory
  and semaphores they look for, with Linux's behaviour, and the libc-test
  cases left out for it run again in the `posix` boot test.
- The boot test's new `shm` program checks sharing by name across
  processes, sizes and SIGBUS, rights, process-shared and named
  semaphores, and System V segments and sets across processes: counts,
  waits, undos, interruptions and removal.
- `df /dev/shm` shows the filesystem's room, which segments share with
  the files.
- **Not yet** (docs/deferred.md):
  - System V message queues (`msgget`); libc-test's `ipc_msg` stays out.
  - `memfd_create`.
  - Folders and symbolic links in `/dev/shm`.
  - Writable shared mappings of other files, which need the page cache
    to map its pages.
  - A memory filesystem for each account or view, as IPC namespaces.
  - `ipcs` and `ipcrm`, and `IPC_INFO`, `SHM_INFO`, `SEM_INFO` and the
    `*_STAT` commands that take an index.
  - `SHM_EXEC` with write access, which Radix never maps together.
