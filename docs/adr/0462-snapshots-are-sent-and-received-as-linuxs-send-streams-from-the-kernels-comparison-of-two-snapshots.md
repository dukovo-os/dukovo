<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0462. Snapshots are sent and received as Linux's send streams, from the kernel's comparison of two snapshots

- Status: Accepted
- Date: 2026-11-11

## Context

The roadmap's 15b backs snapshots up to a USB disk or to another
machine, incrementally: after the first copy, only what changed goes.
Snapshots are read-only and kept in `/snapshots` (ADR 0461). Btrfs
subvolumes came with ADR 0138, whose deferred work names `send` and
`receive` for this.

Linux has both: `btrfs send` writes a read-only snapshot as a stream of
commands (make this file, write these bytes, rename, set the owner),
whole or as its changes from a parent snapshot the receiver already
has, and `btrfs receive` replays a stream into a new subvolume. The
format is documented with btrfs-progs (version 1 since Linux 3.6,
version 2 since 6.0) and read the same way by every Linux since. Linux
makes the stream in the kernel (`BTRFS_IOC_SEND`), which compares the
two snapshots' trees and skips every block they share; `btrfs receive`
is a program, and sets the received subvolume's marks with
`BTRFS_IOC_SET_RECEIVED_SUBVOL`.

## Decision

- **The stream is Linux's, version 1.** `subvolume send` writes what
  Linux's `btrfs receive` reads, and `subvolume receive` reads what
  Linux's `btrfs send --proto 1` writes, clones among them. A Dukovo
  snapshot can go to a Linux machine's Btrfs, and back. Version 2
  (compressed data sent as it is, `fallocate`, file flags) is refused
  on reading until a need for it.
- **The kernel compares; a program makes the stream.** The directory
  method `compare` (52) walks the trees of two read-only subvolumes of
  one filesystem, the one that holds a directory and a parent's, and
  writes a record of every item one has and the other has not, or has
  with other data, for a range of keys, as many as the caller's buffer
  takes. Like Linux's tree comparison, it passes over every block the
  two trees share, so an incremental send reads only what changed.
  The items are the format's own, as Linux's `BTRFS_IOC_TREE_SEARCH`
  gives them. Turning them into commands, ordering them and reading
  the files' data is done by the Btrfs crate's `send` module (feature
  `send`), compiled into `subvolume`(8): the kernel gains one walk,
  not a stream generator, and the same code runs in the host tests.
- **Both subvolumes must be read-only, and the caller must hold the
  bypass right on both** (`not_permitted` otherwise), as Linux's send
  needs read-only snapshots and `CAP_SYS_ADMIN`. Read-only, neither
  changes between the calls of one send; the items hold every file's
  names, owners and data, so reading them is an administrator's.
- **The sender orders its commands as a receiver can apply them.** A
  first walk finds the files whose names changed, the ones gone, the
  inode numbers used again by another file (by the inode's
  generation), and the new folders. Folders that move or go, and
  files that lose every name they kept, step aside to an orphan name
  at the top (`o<ino>-<gen>-<n>`, checked against both snapshots'
  names); other removed names go, then folders are made and moved by
  their new depth, then files are renamed or linked into place. A
  second walk, inode by inode, makes the new files, sets extended
  attributes, writes the changed ranges (holes of new files are not
  written), truncates, and sets owners, modes and times, a folder's
  times last of all. The mode is sent again after an owner change that
  cleared a set-ID bit, as Linux's `chown` does. Data goes as writes of
  48 KiB at most; the sender finds no clones.
- **The receiver works through the capability of the subvolume it
  makes.** `subvolume receive` makes a new subvolume in the folder it
  is given, or a writable snapshot of the stream's parent there, opens
  its top, and applies every command with the directory and file
  methods relative to that capability. The crate refuses a path that is
  absolute, empty, has a NUL, or has a `.` or `..` name; the last name of
  every path is taken as it is, never followed if it is a symbolic link
  (`no_follow`). A stream cannot reach outside the subvolume it makes,
  without Linux's `chroot`.
- **A received subvolume is marked, and read-only.** The directory
  method `mark_received` (53) syncs what was written, then sets the
  subvolume's received UUID, the sender's transaction and the times of
  sending and receiving in its root item, and its read-only flag, in one
  transaction, as Linux's `BTRFS_IOC_SET_RECEIVED_SUBVOL` and `btrfs
  property set ro true` do. A subvolume received already is refused
  (`read_only`).
- **A stream's parent is found in the folder received into.** It is
  the subvolume there received as the parent's UUID at the sender's
  transaction, or else the one that is that UUID at the same
  transaction, as Linux's receive finds it by received UUID first. A
  copy of a copy names its parent by the UUID it was received as, so a
  chain of backups goes on from any of its links.
- **A stream that fails leaves nothing.** The half-made subvolume is
  removed, and the error says so. Subvolumes the stream finished
  before stay, each whole and marked.
- **Interoperation is checked both ways.** `ob fixture btrfs-send` has
  the host's Linux fill, change and send two snapshots; the host tests
  receive Linux's streams and check every file, its data, links,
  owners, modes, times and extended attributes, against Linux's own
  list. The host tests' rounds of random changes, from many seeds, send
  each snapshot whole or from the one before, receive it, and compare;
  with `SEND_STREAMS_TO` they keep the streams and lists, and `ob
  fixture btrfs-send --check` has Linux's `btrfs receive` take every one
  on a loop-mounted Btrfs and checks the copies against the lists
  (96 streams of 16 seeds, none different, when this was decided).

## Consequences

- `subvolume send [-p parent] [-f file] subvolume...` and `subvolume
  receive [-e] [-f file] folder` work as Linux's `btrfs send` and
  `btrfs receive` do for these options; `receive --dump` prints a
  stream as Linux's does, for anyone. The backup tool sends with them.
- The kernel holds the comparison's records while it walks, up to
  256 KiB a call, and copies them out after; the program asks again
  from the key after the last.
- The boot test `send-receive` sends a snapshot of `/users` whole down a
  pipe and then as its changes through a file to another Btrfs disk,
  checks that the copy lists as the snapshot does and has the same
  files and access lists, that an incremental stream without its parent
  is refused and leaves nothing, and that a send without an
  administrator, of a writable subvolume or to a terminal is refused;
  Linux's `btrfs check` then reads that disk clean. The lab's
  `storage/send-receive` has the lab PC's Linux receive the machine's
  streams and the machine receive Linux's.
- Extended attributes in the `user.` namespace on device nodes, pipes
  and sockets go in a stream, as Dukovo keeps them; Linux refuses to
  set them there, so such a stream fails on Linux.
- **Not yet:** version 2 streams; Linux's `-c` clone sources and the
  sender finding clones; finding a parent anywhere on the filesystem
  through its UUID tree; Linux's `--no-data` and `--max-errors`.

## Alternatives considered

- **The stream made in the kernel, as Linux makes it.** A large
  generator, its ordering rules and its buffers in the kernel, for no
  gain: the program reads the same items through one method, and the
  kernel's part is the walk that only it can do without reading every
  block.
- **A walk of both snapshots' files, as rsync compares.** It reads
  every file's metadata on both sides, sees no shared blocks, and
  cannot tell a rename or a hard link from a new file without reading
  data.
- **The program reading the disk itself.** It would need the raw
  device, a reader of its own for the trees, and the running
  transaction's view of them, which only the filesystem holds.
- **A format of Dukovo's own.** Nothing could receive it but Dukovo.
  Backups to the cloud, with restic, are a separate step (15b's next
  part); a Btrfs disk or machine at the other end gets Linux's format.
