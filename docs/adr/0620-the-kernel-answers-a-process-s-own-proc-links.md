<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0620. The kernel answers a process's own /proc links

- Status: Accepted
- Date: 2026-11-24

## Context

Linux's `/proc/self/exe`, `cwd`, `root` and `fd/<n>` are links that lead
not to a path but to the object: `/proc/self/fd/0` reaches a pipe, which has
no path, and `/proc/self/exe` the running program even after its file was
removed. bash's process substitution hands a command `/dev/fd/63`; `cat
/dev/stdin` reads a pipe; programs find their own files from
`/proc/self/exe`. linuxd serves `/proc` in a Linux view (ADR 0621) but
cannot give a program its own descriptors back through a path, and a path
it gave could lead elsewhere in the view than the object it stands for.

## Decision

- **Which paths.** Before any walk, the Linux table looks at an absolute
  path: `/proc/self`, `/proc/thread-self` or `/proc/<the caller's own
  process number>`, then `exe`, `cwd`, `root` or `fd/<n>` and nothing after
  (a trailing slash only after `cwd` and `root`), and `/dev/fd/<n>`,
  `/dev/stdin`, `/dev/stdout` and `/dev/stderr`, are the kernel's. The
  parsing is `radix_linux::magic`'s, with host tests. A link of another
  process's, and one met part-way through a walk, are linuxd's, served as
  plain links to paths.
- **readlink** gives what Linux gives: the path the program was started by,
  from the root; the working directory; `/`; and for a descriptor the path
  it was opened by (`/memfd:NAME (deleted)` for a memory file),
  `pipe:[n]`, `socket:[n]`, `anon_inode:[eventpoll]` for an event queue,
  `anon_inode:[eventfd]`, `[timerfd]`, `[signalfd]` and `[pidfd]` for the
  kernel's own files (ADR 0613), `anon_inode:inotify` for a watch file, and
  `anon_inode:[kind]` for any other object. A
  program whose path is not known (started from a descriptor) has no `exe`
  link: ENOENT.
- **A call that follows a last link** (`open`, `openat`, `stat` and its
  kin, `access`, `faccessat`) goes where it leads: `exe`, `cwd` and `root`
  by their paths, and `fd/<n>` to the descriptor itself. Opening one gives a
  new descriptor of the same object, with the access asked for if the
  descriptor's capability has it (EACCES if not), close-on-exec as asked,
  and ENOTDIR for `O_DIRECTORY` on a file; `stat` gives the descriptor's
  object's status; `access` checks the asked access against the
  descriptor's rights.
- **Differences from Linux.** `exe` is opened again by its path, so a
  program whose file was replaced since it started opens the new file
  (Linux opens the one running). Opening `fd/<n>` shares the descriptor's
  open file description, its offset and status flags, as `dup` does; Linux
  makes a new description for a regular file. Both are rows of
  docs/deferred.md.

## Consequences

- `/dev/stdin`, bash's `/dev/fd/63` and `readlink /proc/self/exe` work in a
  Linux view, and outside one too, since they need nothing of linuxd's.
- The kernel keeps the path a program was started by and each descriptor's
  opening path, which it already did for `/processes` (radix-proc(5)).
