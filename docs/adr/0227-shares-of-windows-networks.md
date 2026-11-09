<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0227. Shares of Windows networks: smb, on smb2, served through FUSE

- Status: Accepted
- Date: 2026-11-02

## Context

Phase 13f.1b. Folders shared on a Windows network, by Windows PCs, by
Samba and by nearly every NAS, are reached with SMB 2 and 3; SMB 1 is
insecure, and Windows no longer turns it on. Linux mounts them with
`cifs`, in its kernel, and `mount -t cifs`, whose options people's
guides and mount tables are full of. Dukovo's kernel keeps the protocol
out: a program serves the share through `/devices/fuse` (ADR 0226).

The client needs SMB 2.1 to 3.1.1, NTLMv2 sign-in, signing and
encryption, which servers ask for more and more, and is best in Rust,
whose std Dukovo has (ADR 0070). libsmbclient is Samba's, in C and
GPL-3.0, with Samba's dependencies after it. The `smb` crate covers the
protocol but reads one request at a time. smb2 (MIT OR Apache-2.0) is
pure Rust, pipelines its requests, signs as each dialect does
(HMAC-SHA256, AES-CMAC and AES-GMAC) and encrypts with AES-128 and
AES-256 in CCM and GCM, signs in with NTLMv2 or Kerberos, and has been
released often through 2025 and 2026. It runs on tokio, or smol.

## Decision

- **smb(1)** (userspace/programs/smb) is the client, in every image:
  `smb mount [-f] [-o options] //server/share[/folder] dir` signs in,
  connects to the share and mounts it, or a folder of it, with
  `mount_fuse`, then serves it in a process of its own in the
  background, or in the foreground with `-f`, until it is unmounted;
  `smb shares //server` lists the server's shares of files, and `smb ls
  //server/share/folder` a folder. A share is named as Linux and Windows
  name one: `//server/share`, `\\server\share` or `smb://server/share`,
  with `:port` after the server.
- **smb2 0.39 on tokio**, on a runtime of one thread, which the
  filesystem's one thread waits on for each call. tokio 1.54.1 and
  socket2 0.6.5 are patched for Radix in the rust-crates port (ADR
  0080): Radix is Linux to them, but for what its sockets lack
  (source-specific multicast, IP_RECVTOS, TCP keepalive's interval and
  count), and mlibc's `msghdr`. What smb2 keeps to itself, reads and
  writes at an offset through a handle the filesystem holds, and cutting
  a file short, go out as SMB 2's own CREATE, READ, WRITE, SET_INFO,
  FLUSH and CLOSE on the tree's connection, which signs or encrypts them
  as it does smb2's. Signing and encryption are what the server asks
  for. A connection that drops is made again as the share is used. A
  DFS namespace is refused for now.
- **Signing in**, in mount.cifs's words: `user=` (with `domain/` and
  `%password`), `password=`, `domain=`, `credentials=` (a file of
  `username=`, `password=` and `domain=` lines), or `guest`; else
  `$USER`, `$PASSWD`, and the password asked for on the terminal without
  showing it. The options mount.cifs takes that change nothing here
  (`vers=`, `sec=`, `_netdev`, `nofail` and the like) are taken, so that
  a line written for Linux works.
- **The files** are shown as Linux's cifs shows a share without the
  POSIX extensions: the mounting user's, or `uid=`'s and `gid=`'s, with
  `file_mode=` or `dir_mode=` (0755), less the write bits of a file the
  server marks read-only. The kernel checks those modes (ADR 0226). A
  chmod that takes the write bits away marks the file read-only, and one
  that gives them back unmarks it, and a file made without them is made
  read-only, as cifs makes one; an owner's change is EPERM, and so
  are links, device nodes and FIFOs. A node number stands for a path
  until the kernel forgets it, and a rename moves the paths beneath;
  what the kernel is told it keeps for a second, since other clients
  change the share too. A folder is listed when it is opened. Each READ
  and WRITE carries 64 KiB at most, what one credit pays for, which the
  filesystem gives INIT as its `max_write`.
- **mount(8) and umount(8)** join systools, as Linux's: `mount` alone
  lists the mounts (`-t` picks kinds, a share's under `smb`, `cifs` or
  `fuse`); `mount -t smb`, `-t cifs`, or a source that starts with `//`,
  runs `smb mount` with the options; any other source is a disk, or its
  name under /devices, mounted with `directory::mount` as the kind `-t`
  names, or as whichever it holds, read-only when the kernel does not
  write it, as Linux's mount does with a warning. `umount` takes a
  folder, or a source mounted. Mounting needs the administrator, as on
  Linux. Volumes that come and go stay volumed(8)'s.
- **Tested** by the `smb` boot test, against the host's Samba run as the
  host's user (no root), with mandatory signing, which the guest reaches
  at 10.0.2.2 through QEMU's user network; smbtest lists shares, mounts
  with `mount -t cifs`, uses the files with the utilities, marks one
  read-only, mounts a folder only to read, unmounts both, and mounts the
  case's FAT disk with mount(8). The options and the node table have unit
  tests on the host. `ob doctor` checks for Samba, and CI's boot tests
  install it.

## Consequences

- A share is a folder every program uses, and the lines Linux mounts a
  share with mount it here.
- A fault in parsing what a server sends ends smb and its mount, not the
  system (ADR 0226).
- **Not yet:** pipelined reads and writes and a page cache for big files
  (one 64 KiB request at a time is slower than Linux's cifs over a fast
  network); leases and oplocks; Kerberos sign-in, which smb2 has; DFS;
  SMB 3.1.1's POSIX extensions with Samba; access lists, for the
  Security tab (13f.3); change notices; symbolic links stored as reparse
  points; mounts by users who are not administrators; and the server
  side, `smbd` and `share` (13f.2).
