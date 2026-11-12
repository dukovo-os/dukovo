<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0465. Backups can be restic repositories, on a share, over SSH or in S3 storage

- Status: Accepted
- Date: 2026-11-12

## Context

Backups (ADR 0463) keep each place as a chain of encrypted send
streams: small once the first is sent, readable by Linux's own tools,
but not deduplicated, and needing a filesystem or a POSIX shell at the
other end. The roadmap's 15b adds restic-format destinations: a NAS's
share, an SFTP server and S3-compatible storage, deduplicated,
encrypted, and append-only where the remote allows.

restic's repository format is specified and widely read. A repository
is a folder of files with fixed names: `config`, `keys/`,
`snapshots/`, `index/` and packs in `data/<two hex digits>/`. Every file
but the keys is encrypted with AES-256 in counter mode and authenticated
with Poly1305 under a master key that each key file wraps with a
password; data is cut into chunks by content, so a chunk any snapshot
already holds is stored once, and since version 2 compressed with zstd.
restic itself is written in Go. rustic is its Rust reimplementation,
whose library, `rustic_core` (Apache-2.0 or MIT), reads and writes the
same format through a backend trait of a few calls: list, read whole,
read part, write, remove.

S3 is a set of HTTP calls that most object stores answer (Amazon's,
Backblaze B2, Wasabi, Cloudflare R2, MinIO, Garage, Ceph), each request
signed with AWS's Signature Version 4.

## Decision

- **A destination's `format` is `btrfs` or `restic`.** `btrfs`, the
  streams of ADR 0463, stays the default for folders and SSH; `restic`
  keeps the same places in a restic repository. An S3 destination is
  always `restic`. `backup add --format restic` chooses it, and
  `--append-only` sets `append_only` (backup.toml(5)).
- **The repository is the machine's folder.** It is
  `dukovo-backup/<machine>` at the destination, as for streams, laid out
  as restic lays one out, with `backup.toml` beside its files to say
  that the folder is Dukovo's, of which machine, in which format (2),
  and with which key. Its password is the destination's recovery key,
  the text `backup add` shows, so the one thing a person keeps for a
  backup still opens it.
- **rustic's library writes it, through backup's own stores.** The
  stores that hold streams, a folder and SSH, and a new one for S3,
  gain ranged reads, sizes and a listing of a folder's files, and
  rustic's backend calls go to them. The xattr crate gains Radix's
  calls, the nix crate leaves out what Radix lacks, and dirs-sys no
  longer brings option-ext, whose MPL-2.0 no system program may link
  (kernel/deny.toml), as patches to the ports' crates.
- **A share is a folder.** A NAS's SMB share or an NFS export is
  mounted (`mount -t cifs`, `mount -t nfs`) and named by its folder.
- **SSH needs a shell, not only SFTP.** The SSH store runs `cat`,
  `dd`, `find`, `mkdir`, `mv` and `rm` on the server, as for streams;
  a server that offers only SFTP, without a shell, is left for later.
- **S3 is reached by Dukovo's own client.** A destination
  `s3://host[:port]/bucket[/prefix]` is reached over HTTPS, and
  `s3+http://` over plain HTTP, for a store on the local network;
  addresses are path-style. Each request is signed with SigV4, the
  body's SHA-256 included. `backup add` takes the access key from
  `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and `AWS_REGION` (or
  `AWS_DEFAULT_REGION`; `us-east-1` without one), as Amazon's tools
  do, and keeps it in `/system/settings/backup/<name>.s3`, readable by
  root alone. S3 has no rename: a file is written with one PUT, which
  is whole or absent.
- **Each place is one snapshot a round.** The round's read-only
  snapshot of each place (ADR 0463) is read and backed up as a restic
  snapshot whose path is the place's path, whose host is the machine's
  name, whose time is the round's, and whose tags are the round's time
  and the place's folder. Its top folder carries the place's own owner,
  mode and times. The machine's record (ADR 0464) is a snapshot of
  `/dukovo-machine`, holding `record.toml`, tagged with the time and
  `machine`. A file whose size, times and inode are those it had in
  the place's previous snapshot is taken from that snapshot without
  being read again.
- **Rounds forget, unless the destination is append-only.** After a
  round, the newest snapshot of each place is kept, with one a day for
  `daily` days, one a week for `weekly` weeks and one a month for
  `monthly` months (7, 4 and 12 by default), as restic's `forget`
  keeps them; the others are forgotten and the data only they used is
  pruned. With `append_only = true`, backup never removes a file from
  the repository, which also says so in its `config`, so a key that can
  write and read but not delete is enough: an S3 key without
  `s3:DeleteObject`, a bucket with object lock, or a share that keeps
  previous versions. A machine broken into cannot then remove its
  backups.
- **Verification is restic's check, then a restore.** `backup verify`
  checks the repository's structure and reads every pack through,
  checking each blob's MAC and hash. With `--restore`, as for streams,
  it restores the newest backup of each place into a scratch subvolume
  and compares it with the snapshot it was made from.
- **Restoring reads the same snapshots.** `backup restore` restores a
  place's snapshot into a subvolume and keeps it as `restored-<time>`
  in the place's `@snapshots` folder; `backup restore --machine`
  stages a whole machine from the newest record and the backups of its
  time, as ADR 0464 does from streams.

## Consequences

- Backups are deduplicated within a place and across places and
  rounds, and compressed. Many rounds cost little more than one.
- A NAS, any server with SSH, and any S3-compatible store hold the
  same kind of backup, and Linux's `restic` (version 0.14 or later)
  and `rustic` read it with the recovery key as the password:
  `restic -r <folder> snapshots`, `restic -r s3:https://host/bucket/
  dukovo-backup/<machine> restore latest --path /users --target .`.
- No one without the recovery key can make data the repository takes
  for its own: restic's MACs come from the master key that the
  password unwraps. The tags of ADR 0464 are not needed for this format.
- A round reads every file's metadata, and the files that changed,
  where a send stream comes from the kernel's comparison of two
  snapshots. For a large place with few changes, streams are quicker.
- Linux's restic restores a place's top folder with its own owner and
  mode, not the place's; Dukovo's restore keeps the place's.
- An append-only repository only grows. Pruning it is a job for whoever
  holds a key that can delete.
- systools, the program that holds backup, grows by about 6 MB of
  code, rustic's library and the crates it brings (chunking, packing,
  globs, a thread pool), to about 21 MB.

## Alternatives

- **restic's own program:** the reference, but in Go, which Dukovo does
  not build, and a second program to keep up to date beside the
  system's own tools.
- **Our own reader and writer of the format:** fewer crates, but a
  format with packs, indexes, trees and chunking to get exactly right;
  rustic's is tested against restic's repositories by its project.
- **restic's REST server as the only remote:** simple, and append-only
  by design, but it needs a server running it, where SMB, SSH and S3
  are already everywhere.
- **An S3 crate:** they bring an async runtime and a TLS stack of their
  own. The calls a repository needs are five, over the HTTP client
  and TLS Dukovo already has for updates.
- **Each destination's own repository password:** one more secret to
  keep. The recovery key is already the one secret a backup needs.
