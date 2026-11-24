<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0619. A FUSE connection's filesystem can be served into a view without a mount

- Status: Accepted
- Date: 2026-11-24

## Context

linuxd serves `/proc`, `/sys` and its netlink sockets as a FUSE filesystem
(ADR 0226). A program's Linux view (ADR 0622) attaches them at `/proc` and
`/sys`. Mounting through `mount_fuse` needs the mount right on a directory
of a tree, which neither linuxd nor the person running a Linux program has,
and a mount in a shared tree would be seen by every program that walks it.
A view needs only a capability to a directory, as it attaches anything else
(ADR 0215).

## Decision

- radix-directory(2) `serve_fuse` (57, version 14) takes a FUSE connection's
  slot and a `to` slot. It makes the filesystem the connection serves on a
  mount that is in no tree, as a resource domain's `/processes` is, queues
  `INIT` at once as `mount_fuse` does, and puts a capability to its root in
  `to`, acting for this capability's name, with the read, write, copy and
  transfer rights, never control.
- It needs no right on the directory it is called on, since it adds nothing
  to any tree: the root it gives is reached only through that capability,
  as a view's directories are. ADR 0226's rules hold: no devices, no
  set-user-ID, the kernel checks owners and modes, and names hold no
  slashes.

## Consequences

- linuxd's supervisor attaches `/proc` and `/sys` in a program's view
  without any right over the system's trees, and no other program sees
  them.
- A filesystem served this way ends when the last capability to it goes,
  or its connection closes, as a FUSE mount does.
