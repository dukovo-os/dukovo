<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0352. A slash or a dot after a file is not a directory, and mkdir of a dot finds one

- Status: Accepted; amends [0081](0081-symbolic-links.md) and [0498](0498-absolute-links-beneath-directory-descriptors-a-named-pipe-s-identity-trailing-slashes-and-close-range.md)
- Date: 2026-11-20

## Context

POSIX.1-2024 (XBD 4.16) resolves a path that ends in `/` only if its
last component names a directory, or a symbolic link to one, which the
slash makes the walk follow. `.` and `..` are entries of a directory,
so `file/.` and `file/..` are not there. Linux fails each with ENOTDIR,
whatever the call. A path whose last component is `.` or `..` names a
directory that already exists, so Linux's `mkdir`, `link`, `symlink`
and `mknod` fail on it with EEXIST, and a new name with a slash after it
that is not a directory's fails with ENOENT.

The kernel's walk (ADR 0081) followed a symbolic link with a slash after
it, but did not check what the walk ended on, and skipped `.` without
looking at what it stood in. The operations that make or remove an entry
cut the slashes off the last name and went on without them. ADR 0498
changed only `open` with `O_CREAT`. So `stat("file/")`, `open("file/.")`,
`unlink("file/")`, `rename("file/", ...)` and `link("file/", ...)` acted
on the file. `rmdir("link/")`, where the link leads to a directory,
removed the link. `mkdir("dir/.")` failed with EINVAL, which `mkdir -p`
and Go's `os.Root.MkdirAll` do not take for a directory that is there.

PHP's test suite found the first gap (ADR 0499). Go's `os.Root` tests
found both (ADR 0350). They compare each call made through an
`os.Root`, which walks a path one name at a time with `openat`, with the
same call made by path, over every way of naming a file, a directory and
a link.

## Decision

- **A walk fails with `not_directory` at a `.` or `..` that follows a
  file**, as it does at a name that follows one. It checks the place it
  is at before it steps through `.` or back through `..`, in the walk
  without `..` and links as well as in the one with them.
- **A walk whose path ends in `/` fails with `not_directory` unless it
  ends on a directory.** A symbolic link there has been followed, so a
  link to a file with a slash after it fails, and one to a directory is
  the directory. `stat`, `lstat`, `open`, `access`, `chmod`, `realpath`
  and the other calls that resolve a whole path all go through it.
- **The calls on a last name check its slash themselves.** `remove`
  (the C library's `unlink` and `rmdir`) and `remove_subvolume` fail with
  `not_directory` when the entry, a symbolic link not followed, is not a
  directory. `rename` does so when a slash follows either name and the
  entry it moves is not a directory, as Linux does.
- **A new entry is made only where its path names a new entry.** For
  `make_directory`, `make_link`, `make_node`, `bind_socket`, `link`'s
  new name, `make_subvolume` and `snapshot`, a path whose last component
  is `.` or `..` is walked whole: it fails with `exists` when the walk
  ends on a directory, and otherwise as the walk fails. A new name that
  is not a directory, with a slash after it, fails with `not_found` once
  the name is found not to be there, as Linux has it.
- **`open` with `O_CREAT` checks each path it would make a file at**:
  the path, and the target that replaces its last name at each symbolic
  link, fails with `is_directory` when a slash follows it (ADR 0498 had
  only the path) or when its last component is `.` or `..` and leads to
  a directory, as Linux does.
- No interface changes: every error is one the calls already raise.

## Consequences

- A program sees Linux's errors for a slash or a dot after a file, and
  `mkdir -p` and `MkdirAll` succeed on a path that ends in `.` or `..`.
- The checks cost a kind comparison where a path has a `.` or `..`
  after a name, or a slash at its end; a walk of plain names does no
  more than before.
- Go's `os.Root` tests on slashes and dots leave `userspace/go/skips.txt`.
  Radix now answers as Linux does where platforms differ, on `mkdir`,
  `rmdir` and `rename` of a link with a slash after it, which none of
  them follows. Go's patch for Radix adds `radix` beside `linux` where
  `os.Root` and its tests choose by platform.
  The PHP tests that `lab/tests/services/php-skips.txt` skips for a slash
  after a file wait for the lab's next run of PHP's suite to leave it.
- `unlink(".")`, `rmdir(".")` and `rename` of a `.` or `..` still fail
  with EINVAL, where Linux gives EISDIR, EINVAL and EBUSY: POSIX allows
  EINVAL for them, and no program has needed Linux's.
- The VFS's tests check each case, and the `posix` boot test's `files`
  program checks them through the C library.
