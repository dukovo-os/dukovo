<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0498. Absolute links beneath directory descriptors, a named pipe's identity, trailing slashes and close_range

- Status: Accepted; amends [0081](0081-symbolic-links.md)
- Date: 2026-11-13

## Context

CPython's test suite, which the lab runs whole (ADR 0495), found four
more calls that fail on Radix and work on Linux, each of which programs
meet:

- A symbolic link whose target is an absolute path, met by a call
  beneath a directory descriptor (`openat`, `fstatat`, `fchownat`,
  `mkdirat` and the rest with a `dirfd`), was followed from that
  directory, as ADR 0081 chose for a walk beneath any capability: a link
  to `/tmp/x/dir` led to `<dirfd>/tmp/x/dir`, which is not there
  (ENOENT). On Linux the target is resolved from the process's root
  wherever the walk began. `os.fwalk` stats each entry with its parent's
  descriptor and took a link to a directory for a file, and
  `shutil.chown(path, dir_fd=...)` failed.
- `fstat` of a named pipe opened by its path gave the pipe's own number
  as `st_ino`, and its own size, times and link count, where `lstat` of
  the path gives the file's. Python's `os.fwalk` and `shutil.rmtree`
  compare the two (`os.path.samestat`) to be sure that a path still
  leads to what they opened, and a walk that met a named pipe stopped
  or refused there.
- `open` with `O_CREAT` of a path that ends in `/` made a file of the
  last name. On Linux it fails with EISDIR before anything is looked up,
  whether or not the name exists.
- mlibc has no `close_range`. CPython's `subprocess` closes a child's
  descriptors with it where it knows it to be async-signal-safe, and
  otherwise lists `/proc/self/fd`, which Radix does not have, and then
  closes each number up to the soft `RLIMIT_NOFILE`; descriptors above a
  limit that was lowered stay open in the child.

## Decision

- **A directory capability opened with the new `absolute_links_leave`
  flag fails a walk at a symbolic link with an absolute target**, with
  `outside` (EXDEV), as it does for a `..` above the directory. The flag
  is `OpenFlags::absolute_links_leave` of radix-directory(2), which is
  now at version 10. It is kept in the description's private flags and
  only for a directory: the root capability Spark and the loader give a
  process has none, so absolute links from `/` and a sandbox's own links
  behave as before. A file made through a dangling absolute link fails
  the same way. `Who`, the second gate, carries it into the walk
  (ADR 0214).
- **The C library asks for the flag on every `open` it makes, and when a
  call beneath a directory descriptor fails with EXDEV and it knows the
  descriptor's path (ADR 0089), makes the call again from the root** with
  `<the directory's path>/<the path>`. The kernel then follows the link
  from the root the process holds. This is one helper, `radix::at` (and
  `radix::at2` for the two paths of `renameat` and `linkat`), which every
  call with a `dirfd` goes through: `openat`, `fstatat`, `faccessat`,
  `mkdirat`, `unlinkat`, `renameat`, `readlinkat`, `linkat`, `symlinkat`,
  `fchmodat`, `fchownat`, `utimensat` and `mknodat`. `O_NOFOLLOW`,
  `AT_SYMLINK_NOFOLLOW` and the link that is the last name of `unlinkat`,
  `renameat` and `readlinkat` are never followed, so they never meet the
  target.
- **`fstat` of a named pipe's end says what `lstat` of its path says**:
  `st_dev`, `st_ino`, the mode, owner, group, times and number of names,
  with 0 for `st_size`. The end keeps the node of the path it was opened
  by and reads it. An anonymous pipe is as it was.
- **`open` with `O_CREAT` of a path that ends in `/` fails with EISDIR**
  before anything is looked up, whether or not the name exists.
  `O_CREAT | O_DIRECTORY` stays EINVAL, and without `O_CREAT` nothing
  changes.
- **`close_range(first, last, flags)`** is declared in `<unistd.h>` with
  `_GNU_SOURCE`, as Linux 5.9 and glibc 2.34 have it, through a new
  `CloseRange` sysdep (mlibc patch 0078). It closes each open descriptor
  from `first` to `last`, whatever `RLIMIT_NOFILE` says, or with
  `CLOSE_RANGE_CLOEXEC` sets close-on-exec on them. `CLOSE_RANGE_UNSHARE`
  is taken and does nothing, as no process shares its descriptors with
  another. A `first` above `last` or any other flag is EINVAL. It walks
  the C library's table of taken descriptors with atomic operations, so,
  like `close` and `dup2` (ADR 0089), it holds no lock and allocates
  nothing, and a child may call it between `fork` and `exec`. CPython's
  `_posixsubprocess.c` takes it as async-signal-safe on Radix (Python
  patch 0005).

## Consequences

- No walk leaves a capability it could not leave before. The kernel
  only fails a walk at an absolute link; the C library's retry goes from
  the root capability the process already holds, by a path it could
  have given itself, so it reaches nothing more. A capability the
  process was given by another (its root, a sandbox's folders) has no
  flag, and absolute links beneath it resolve inside it as before (ADR
  0081's `RESOLVE_IN_ROOT`).
- A descriptor the C library has no path for (one that came from before
  `exec`, or opened while the pool of paths was full) keeps EXDEV at an
  absolute link, as it does for a `..`. One whose directory was renamed
  or moved while it was open goes by the old path, as a `..` beneath it
  does (ADR 0089): the call made again from the root may meet another
  link there, or none, where Linux reads the link in the directory.
- An error path may make one more call: an `at` call that fails with
  EXDEV for another reason, a `rename` or `link` across mounts, is made
  again from the root and fails the same way.
- `close_range` is the C library's: it closes what the C library has
  marked taken. A slot that something else filled, and no call has yet
  found occupied, is not marked and is not closed by it
  (radix-posix(7)).
- `os.fwalk`'s tests, `test_rmtree_on_named_pipe`, `test_chown`,
  `test_copyfile_nonexistent_dir` and
  `test_close_fds_when_max_fd_is_lowered` leave
  `lab/tests/services/python-skips.txt`. `scandir`'s file numbers
  (`d_ino`) stay in deferred work. `test_cleanup_with_symlink_race`
  stays skipped for another reason: `tempfile` sets a file's mode
  through a descriptor open only for reading, which ADR 0213 refuses.
- The `posix` boot test's `files` program checks each: links beneath a
  directory descriptor, a copy of it, one from `opendir` and one in a
  child; a named pipe's `fstat` against its `lstat`; the three trailing
  slash cases; and `close_range` with each flag, in a child with a lowered
  limit.
