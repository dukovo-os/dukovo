<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0615. File watches follow a node, and its directory's changes reach them

- Status: Accepted
- Date: 2026-11-23

## Context

Programs that follow files, `tail -f`, build tools, editors, service
managers reloading their settings and language servers, ask Linux's
inotify to tell them when a file or a directory's entries change, instead
of looking again and again. An inotify watch is of an inode, not a path:
it follows the file across renames, and a directory's watch hears of
changes to the entries in it. Radix had no way for a program to learn of
a change but to look.

## Decision

- **A watch file** (`fs/watch_file.rs`) is a node of the pipes'
  filesystem, as the kernel's other files are (ADR 0613), made by
  radix-process(2)'s `watch_file` in a slot. radix-file(2)'s `watch_add`
  and `watch_remove` (version 14) add and take away its watches, and its
  reads give Linux's `struct inotify_event` records, as many whole ones
  as fit, failing with invalid_argument if the first does not. It is
  readable while it holds an event and never writable. It holds
  `WATCH_QUEUE_MAX` (16,384) events, and then one `IN_Q_OVERFLOW` for
  those lost; an event equal to the last queued is not queued again.
  `FIONREAD` gives the bytes waiting.
- **A watch is of a node**, found as `open` finds it: the last name is
  followed unless `IN_DONT_FOLLOW`, `IN_ONLYDIR` refuses anything but a
  directory, and the caller must be able to read it. The watch holds the
  node, as Linux's pins the inode, so the node keeps its address, which is
  the key of a table of all watches (`fs/notify.rs`). A file watched
  already by the same watch file keeps its number and takes the new
  events (`IN_MASK_ADD` adds them; `IN_MASK_CREATE` fails with exists). A
  watch file holds at most `WATCHES_MAX` (8,192) watches.
- **A change reaches the node's watches and its directory's.** The hooks
  of the directory methods (`user/notices.rs`) tell what was made,
  removed, moved and changed once the change is made, and the file
  methods tell opens, reads, writes and closes through the directory the
  file was opened in. A change that removes or replaces a name looks
  first at what the name led to, with a second walk made only while
  something is watched. A move gives its two records one cookie. A
  removed node's watches get `IN_DELETE_SELF` and then `IN_IGNORED`, as do
  `IN_ONESHOT` watches once they fire.
- **Every hook costs one load while nothing is watched** (`WATCHED`). The
  table's lock is a spin lock, under which the watches to tell are only
  collected: the event is built before it is taken, and queued, with
  memory freed and threads woken, after it is let go, so no other lock is
  ever taken under it.
- **The Linux table** gives `inotify_init1`, `inotify_add_watch` and
  `inotify_rm_watch`, with the `IN_` bits of `radix_linux::notify`, which
  has host tests. Dukovo's C library has the same three (mlibc patch 0120
  takes inotify out of its Linux option, which needs Linux's headers).

## Consequences

- A watch follows a file across renames, as Linux's does, and a program
  that watches a path it then renames over keeps watching the old node.
- Changes made below the kernel's directory methods are not seen: a file
  changed by another machine on a network filesystem, or by a filesystem
  image mounted twice.
- Ports' configure scripts that look for inotify find it; none of the
  ports built now does.
- The `posix` boot test runs `inotify.c` natively and under
  `linux-programs`: creation, deletion and modification seen from the
  directory, a move with one cookie, `IN_CLOSE_WRITE`, `IN_IGNORED` on
  removal, `IN_ONESHOT`, and EINVAL for a read too small.
